package io.github.aabolfazl.perfagent.engine

import io.github.aabolfazl.perfagent.proto.ConnectionState
import java.io.IOException
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.DatagramChannel
import java.nio.channels.SelectionKey

/** One app UDP "connection" (source and destination pair), forwarded over a connected datagram socket. */
internal class UdpFlow(
    private val engine: Engine,
    val key: FlowKey,
    val uid: Int,
    val record: ConnectionRecord,
) : Selectable {
    private var channel: DatagramChannel? = null
    private var selectionKey: SelectionKey? = null
    private var closed = false
    private val isDns = key.dstPort == 53

    fun start(): Boolean {
        return try {
            val ch = DatagramChannel.open()
            channel = ch
            ch.configureBlocking(false)
            if (!engine.platform.protect(ch.socket())) {
                close("Could not protect socket")
                return false
            }
            ch.connect(engine.platform.upstreamAddress(InetSocketAddress(intToIpv4(key.dstIp), key.dstPort)))
            selectionKey = ch.register(engine.selector, SelectionKey.OP_READ, this)
            record.state = ConnectionState.OPEN
            true
        } catch (e: IOException) {
            close(e.message ?: "Could not open socket")
            false
        }
    }

    fun send(data: ByteArray, offset: Int, length: Int) {
        val ch = channel ?: return
        try {
            ch.write(ByteBuffer.wrap(data, offset, length))
        } catch (e: IOException) {
            close(e.message ?: "Send failed")
        }
    }

    override fun onSelected(key: SelectionKey) {
        val ch = channel ?: return
        val buffer = engine.udpBuffer
        try {
            while (true) {
                buffer.clear()
                val n = ch.read(buffer)
                if (n <= 0) break
                if (isDns) engine.tracker.dnsAnswer(uid, Dns.parse(buffer.array(), 0, n), failed = false)
                if (n > engine.segmentSize + TCP_HEADER - UDP_HEADER) {
                    // Larger than the VPN MTU; IP fragmentation isn't implemented.
                    engine.tracker.unsupportedPackets++
                    continue
                }
                val packet = buildUdp(this.key.dstIp, this.key.dstPort, this.key.srcIp, this.key.srcPort, buffer.array(), 0, n)
                engine.writeToTun(packet, record, uid)
            }
        } catch (e: IOException) {
            // For example ICMP port unreachable on a connected socket.
            close(e.message ?: "Receive failed")
        }
    }

    fun onTimer(now: Long) {
        val timeout = if (isDns) DNS_TIMEOUT_MS else IDLE_TIMEOUT_MS
        if (now - record.lastMs > timeout) close(null)
    }

    fun close(error: String?) {
        if (closed) return
        closed = true
        selectionKey?.cancel()
        channel?.let { runCatching { it.close() } }
        engine.removeUdp(this)
        engine.tracker.close(record, if (error == null) ConnectionState.CLOSED else ConnectionState.FAILED, error)
    }

    companion object {
        const val DNS_TIMEOUT_MS = 15_000L
        const val IDLE_TIMEOUT_MS = 120_000L
    }
}
