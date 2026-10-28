package io.github.aabolfazl.perfagent

import android.net.ConnectivityManager
import android.net.DnsResolver
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.os.Process
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import io.github.aabolfazl.perfagent.engine.Platform
import io.github.aabolfazl.perfagent.engine.TunDevice
import io.github.aabolfazl.perfagent.engine.UID_UNKNOWN
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.Executor

internal class AndroidPlatform(private val service: VpnService) : Platform {
    private val connectivity = checkNotNull(service.getSystemService(ConnectivityManager::class.java))
    private val packages = service.packageManager
    private val directExecutor = Executor { it.run() }

    override fun protect(socket: Socket): Boolean = service.protect(socket)

    override fun protect(socket: DatagramSocket): Boolean = service.protect(socket)

    /** Only the active VPN app may ask; it returns INVALID_UID (-1) when the owner is unknown. */
    override fun ownerUid(protocol: Int, local: InetSocketAddress, remote: InetSocketAddress): Int =
        try {
            connectivity.getConnectionOwnerUid(protocol, local, remote)
        } catch (e: SecurityException) {
            UID_UNKNOWN
        }

    override fun appName(uid: Int): String {
        when (uid) {
            Process.ROOT_UID -> return "root"
            Process.SYSTEM_UID -> return "android"
        }
        val names = packages.getPackagesForUid(uid)
        return when {
            names.isNullOrEmpty() -> packages.getNameForUid(uid) ?: "uid $uid"
            names.size == 1 -> names[0]
            // Apps sharing one uid (for example system components).
            else -> names.sorted().joinToString(",")
        }
    }

    /** Uses the phone's own resolver (and Private DNS settings) on the real network. */
    override fun resolveDns(query: ByteArray, reply: (ByteArray?) -> Unit) {
        DnsResolver.getInstance().rawQuery(
            null, query, DnsResolver.FLAG_EMPTY, directExecutor, null,
            object : DnsResolver.Callback<ByteArray> {
                override fun onAnswer(answer: ByteArray, rcode: Int) = reply(answer)
                override fun onError(error: DnsResolver.DnsException) = reply(null)
            },
        )
    }
}

/** The VPN interface, opened non-blocking and polled so the reader thread can be stopped. */
internal class AndroidTun(pfd: ParcelFileDescriptor) : TunDevice {
    private val input = FileInputStream(pfd.fileDescriptor)
    private val output = FileOutputStream(pfd.fileDescriptor)
    private val pollFds = arrayOf(
        StructPollfd().apply {
            fd = pfd.fileDescriptor
            events = OsConstants.POLLIN.toShort()
        },
    )

    @Volatile
    private var closed = false

    override fun read(buffer: ByteArray): Int {
        if (closed) return -1
        return try {
            val ready = Os.poll(pollFds, 200)
            if (closed) return -1
            if (ready <= 0) return 0
            val revents = pollFds[0].revents.toInt()
            if (revents and (OsConstants.POLLERR or OsConstants.POLLHUP or OsConstants.POLLNVAL) != 0) return -1
            // A non-blocking read returns 0 rather than failing when there is nothing to read.
            val n = input.read(buffer)
            if (n < 0) -1 else n
        } catch (e: ErrnoException) {
            if (e.errno == OsConstants.EINTR) 0 else -1
        } catch (e: IOException) {
            -1
        }
    }

    override fun write(buffer: ByteArray, offset: Int, length: Int) {
        output.write(buffer, offset, length)
    }

    fun close() {
        closed = true
    }
}
