package io.github.aabolfazl.perfagent.engine

import com.sun.jna.Library
import com.sun.jna.Memory
import com.sun.jna.Native
import com.sun.jna.NativeLong
import com.sun.jna.Pointer
import java.io.IOException
import java.net.InetAddress

/** A Linux TUN interface for tests, playing the part of Android's VPN file descriptor. Needs root. */
class LinuxTun(name: String, address: String, prefixLength: Int) : TunDevice, AutoCloseable {
    @Suppress("FunctionName")
    private interface C : Library {
        fun open(path: String, flags: Int): Int
        fun close(fd: Int): Int
        fun ioctl(fd: Int, request: NativeLong, arg: Pointer): Int
        fun read(fd: Int, buffer: ByteArray, count: NativeLong): NativeLong
        fun write(fd: Int, buffer: ByteArray, count: NativeLong): NativeLong
        fun socket(domain: Int, type: Int, protocol: Int): Int
        fun poll(fds: Pointer, count: NativeLong, timeoutMs: Int): Int
    }

    private val c = Native.load("c", C::class.java)
    private val fd: Int

    @Volatile
    private var closed = false

    init {
        fd = c.open("/dev/net/tun", O_RDWR)
        if (fd < 0) throw IOException("cannot open /dev/net/tun")
        val ifreq = ifreq(name)
        ifreq.setShort(16, (IFF_TUN or IFF_NO_PI).toShort())
        check(c.ioctl(fd, NativeLong(TUNSETIFF), ifreq) == 0) { "TUNSETIFF failed" }

        val sock = c.socket(AF_INET, SOCK_DGRAM, 0)
        try {
            check(c.ioctl(sock, NativeLong(SIOCSIFADDR), sockaddr(name, InetAddress.getByName(address).address)) == 0) { "SIOCSIFADDR" }
            val mask = (-1L shl (32 - prefixLength)).toInt()
            val maskBytes = byteArrayOf((mask ushr 24).toByte(), (mask ushr 16).toByte(), (mask ushr 8).toByte(), mask.toByte())
            check(c.ioctl(sock, NativeLong(SIOCSIFNETMASK), sockaddr(name, maskBytes)) == 0) { "SIOCSIFNETMASK" }
            val flags = ifreq(name)
            check(c.ioctl(sock, NativeLong(SIOCGIFFLAGS), flags) == 0) { "SIOCGIFFLAGS" }
            flags.setShort(16, (flags.getShort(16).toInt() or IFF_UP).toShort())
            check(c.ioctl(sock, NativeLong(SIOCSIFFLAGS), flags) == 0) { "SIOCSIFFLAGS" }
        } finally {
            c.close(sock)
        }
    }

    override fun read(buffer: ByteArray): Int {
        if (closed) return -1
        val pollfd = Memory(8).apply {
            setInt(0, fd)
            setShort(4, POLLIN.toShort())
            setShort(6, 0)
        }
        val ready = c.poll(pollfd, NativeLong(1), 200)
        if (closed) return -1
        if (ready <= 0) return 0
        val n = c.read(fd, buffer, NativeLong(buffer.size.toLong())).toInt()
        return if (n < 0) -1 else n
    }

    override fun write(buffer: ByteArray, offset: Int, length: Int) {
        val data = if (offset == 0) buffer else buffer.copyOfRange(offset, offset + length)
        if (c.write(fd, data, NativeLong(length.toLong())).toInt() != length) throw IOException("TUN write failed")
    }

    override fun close() {
        closed = true
        Thread.sleep(300) // let a reader blocked in poll() notice
        c.close(fd)
    }

    private fun ifreq(name: String) = Memory(40).apply {
        clear()
        val bytes = name.toByteArray()
        write(0, bytes, 0, bytes.size)
    }

    private fun sockaddr(name: String, address: ByteArray) = ifreq(name).apply {
        setShort(16, AF_INET.toShort())
        write(20, address, 0, 4)
    }

    private companion object {
        const val O_RDWR = 2
        const val AF_INET = 2
        const val SOCK_DGRAM = 2
        const val POLLIN = 1
        const val IFF_TUN = 0x0001
        const val IFF_NO_PI = 0x1000
        const val IFF_UP = 0x1
        const val TUNSETIFF = 0x400454caL
        const val SIOCSIFADDR = 0x8916L
        const val SIOCSIFNETMASK = 0x891cL
        const val SIOCGIFFLAGS = 0x8913L
        const val SIOCSIFFLAGS = 0x8914L
    }
}
