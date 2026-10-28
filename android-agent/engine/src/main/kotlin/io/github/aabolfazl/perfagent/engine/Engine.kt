package io.github.aabolfazl.perfagent.engine

import io.github.aabolfazl.perfagent.proto.Snapshot
import java.io.IOException
import java.net.Inet4Address
import java.net.InetSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.SelectionKey
import java.nio.channels.Selector
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicInteger
import java.util.logging.Level
import java.util.logging.Logger

class EngineConfig(
    /** Must match the MTU of the TUN device. */
    val mtu: Int = 1500,
    /** The VPN's DNS server address; queries sent to it are answered through [Platform.resolveDns]. */
    val dnsServer: Inet4Address? = null,
    /** Package the VPN was limited to, reported in snapshots. */
    val appFilter: String? = null,
    val agentVersion: String = "0.1.0",
    val snapshotIntervalMs: Long = 1000,
)

fun interface SnapshotListener {
    fun onSnapshot(snapshot: Snapshot)
}

/** Sees every IP packet in both directions; called on the engine thread, so it must not block. */
fun interface PacketTap {
    fun onPacket(data: ByteArray, offset: Int, length: Int, timeMicros: Long)
}

internal interface Selectable {
    fun onSelected(key: SelectionKey)
}

internal data class FlowKey(val srcIp: Int, val srcPort: Int, val dstIp: Int, val dstPort: Int)

/**
 * Turns the packets apps send into the VPN into ordinary sockets, and the replies back into
 * packets, while measuring everything that passes. One thread reads the TUN device and one
 * thread owns all connection state, so the flow code needs no locking.
 */
class Engine(
    private val tun: TunDevice,
    internal val platform: Platform,
    private val config: EngineConfig = EngineConfig(),
) {
    @Volatile
    var snapshotListener: SnapshotListener? = null

    @Volatile
    var packetTap: PacketTap? = null

    internal val selector: Selector = Selector.open()
    internal val tracker = Tracker(platform, ::now)
    internal val segmentSize = config.mtu - IPV4_HEADER - TCP_HEADER

    private class InPacket(val data: ByteArray, val length: Int, val timeMicros: Long)

    private val incoming = ConcurrentLinkedQueue<InPacket>()
    private val incomingCount = AtomicInteger()
    private val tasks = ConcurrentLinkedQueue<() -> Unit>()
    private val bufferPool = ConcurrentLinkedQueue<ByteArray>()

    private val tcpFlows = HashMap<FlowKey, TcpFlow>()
    private val udpFlows = HashMap<FlowKey, UdpFlow>()
    private val dnsServerIp = config.dnsServer?.let(::ipv4ToInt)
    private val startedMs = now()
    internal val udpBuffer: ByteBuffer = ByteBuffer.allocate(65535)

    @Volatile
    private var running = false
    private var reader: Thread? = null
    private var worker: Thread? = null

    internal fun now(): Long = System.currentTimeMillis()

    private val clockBaseMicros = System.currentTimeMillis() * 1000 - System.nanoTime() / 1000

    /** Wall-clock time in microseconds with sub-millisecond resolution, for packet timestamps. */
    private fun micros(): Long = clockBaseMicros + System.nanoTime() / 1000

    fun start() {
        check(!running) { "already started" }
        running = true
        worker = Thread(::workLoop, "perf-agent-engine").apply { start() }
        reader = Thread(::readLoop, "perf-agent-tun").apply { start() }
    }

    /** Stops both threads and closes every forwarded socket. The TUN device is left to the caller. */
    fun stop() {
        running = false
        selector.wakeup()
        reader?.join(2000)
        worker?.join(2000)
    }

    val isRunning: Boolean get() = running

    /** Runs [task] on the engine thread. */
    internal fun post(task: () -> Unit) {
        tasks.add(task)
        selector.wakeup()
    }

    private fun readLoop() {
        try {
            while (running) {
                val buffer = bufferPool.poll() ?: ByteArray(config.mtu)
                val n = tun.read(buffer)
                if (n < 0) break
                if (n == 0) {
                    bufferPool.offer(buffer)
                    continue
                }
                // Don't let a stalled engine thread eat all memory; TCP will resend.
                if (incomingCount.get() > MAX_QUEUED_PACKETS) {
                    bufferPool.offer(buffer)
                    continue
                }
                incoming.add(InPacket(buffer, n, micros()))
                incomingCount.incrementAndGet()
                selector.wakeup()
            }
        } catch (e: IOException) {
            if (running) log.log(Level.WARNING, "TUN read failed", e)
        }
        running = false
        selector.wakeup()
    }

    private fun workLoop() {
        var nextTimer = now() + TIMER_INTERVAL_MS
        var nextSnapshot = now() + config.snapshotIntervalMs
        try {
            while (running) {
                selector.select(TIMER_INTERVAL_MS)

                while (true) {
                    val packet = incoming.poll() ?: break
                    incomingCount.decrementAndGet()
                    guarded { handlePacket(packet) }
                    bufferPool.offer(packet.data)
                }
                while (true) {
                    val task = tasks.poll() ?: break
                    guarded(task)
                }

                val selected = selector.selectedKeys()
                for (key in selected) {
                    val target = key.attachment() as? Selectable ?: continue
                    if (key.isValid) guarded { target.onSelected(key) }
                }
                selected.clear()

                val now = now()
                if (now >= nextTimer) {
                    nextTimer = now + TIMER_INTERVAL_MS
                    for (flow in tcpFlows.values.toList()) guarded { flow.onTimer(now) }
                    for (flow in udpFlows.values.toList()) guarded { flow.onTimer(now) }
                }
                if (now >= nextSnapshot) {
                    nextSnapshot = now + config.snapshotIntervalMs
                    publishSnapshot()
                }
            }
        } finally {
            running = false
            for (flow in tcpFlows.values.toList()) flow.abort()
            for (flow in udpFlows.values.toList()) flow.close(null)
            publishSnapshot()
            runCatching { selector.close() }
        }
    }

    private inline fun guarded(block: () -> Unit) {
        try {
            block()
        } catch (e: Exception) {
            log.log(Level.WARNING, "Engine error", e)
        }
    }

    private fun publishSnapshot() {
        val listener = snapshotListener ?: return
        val snapshot = tracker.snapshot(config.agentVersion, startedMs, config.appFilter, tcpFlows.size, udpFlows.size)
        try {
            listener.onSnapshot(snapshot)
        } catch (e: Exception) {
            log.log(Level.WARNING, "Snapshot listener failed", e)
        }
    }

    private fun handlePacket(packet: InPacket) {
        packetTap?.onPacket(packet.data, 0, packet.length, packet.timeMicros)
        val ip = IpPacket.parse(packet.data, packet.length)
        when (ip?.protocol) {
            PROTO_TCP -> handleTcp(ip)
            PROTO_UDP -> handleUdp(ip)
            else -> tracker.unsupportedPackets++
        }
    }

    private fun lookupUid(protocol: Int, ip: IpPacket): Int = try {
        platform.ownerUid(
            protocol,
            InetSocketAddress(intToIpv4(ip.src), ip.srcPort),
            InetSocketAddress(intToIpv4(ip.dst), ip.dstPort),
        )
    } catch (e: Exception) {
        UID_UNKNOWN
    }

    private fun handleTcp(ip: IpPacket) {
        val segment = TcpSegment.parse(ip) ?: run {
            tracker.unsupportedPackets++
            return
        }
        val key = FlowKey(ip.src, ip.srcPort, ip.dst, ip.dstPort)
        tcpFlows[key]?.let { flow ->
            tracker.fromApp(flow.record, flow.uid, ip.length)
            flow.onSegment(segment, ip.data)
            return
        }

        val isNewConnection = segment.has(TCP_SYN) && !segment.has(TCP_ACK)
        if (!isNewConnection || tcpFlows.size >= MAX_TCP_FLOWS) {
            tracker.fromApp(null, UID_UNKNOWN, ip.length)
            // A segment for a connection we don't know: reset it so the app stops waiting.
            if (!segment.has(TCP_RST)) sendResetFor(ip, segment)
            return
        }

        val uid = lookupUid(PROTO_TCP, ip)
        val record = tracker.open(PROTO_TCP, uid, ip.srcPort, ip.dst, ip.dstPort)
        tracker.fromApp(record, uid, ip.length)
        val flow = TcpFlow(this, key, uid, record)
        tcpFlows[key] = flow
        flow.start(segment)
    }

    private fun sendResetFor(ip: IpPacket, segment: TcpSegment) {
        val packet = if (segment.has(TCP_ACK)) {
            buildTcp(ip.dst, ip.dstPort, ip.src, ip.srcPort, segment.ack, 0, TCP_RST, 0)
        } else {
            var consumed = segment.payloadLength
            if (segment.has(TCP_SYN)) consumed++
            if (segment.has(TCP_FIN)) consumed++
            buildTcp(ip.dst, ip.dstPort, ip.src, ip.srcPort, 0, segment.seq + consumed, TCP_RST or TCP_ACK, 0)
        }
        writeToTun(packet, null, UID_UNKNOWN)
    }

    private fun handleUdp(ip: IpPacket) {
        val o = ip.l4Offset
        if (ip.length - o < UDP_HEADER) {
            tracker.unsupportedPackets++
            return
        }
        val payloadOffset = o + UDP_HEADER
        val payloadLength = minOf(u16(ip.data, o + 4) - UDP_HEADER, ip.length - payloadOffset)
        if (payloadLength < 0) {
            tracker.unsupportedPackets++
            return
        }

        if (ip.dst == dnsServerIp && ip.dstPort == 53) {
            handleDnsQuery(ip, payloadOffset, payloadLength)
            return
        }

        val key = FlowKey(ip.src, ip.srcPort, ip.dst, ip.dstPort)
        var flow = udpFlows[key]
        if (flow == null) {
            if (udpFlows.size >= MAX_UDP_FLOWS) {
                tracker.fromApp(null, UID_UNKNOWN, ip.length)
                return
            }
            val uid = lookupUid(PROTO_UDP, ip)
            val record = tracker.open(PROTO_UDP, uid, ip.srcPort, ip.dst, ip.dstPort)
            flow = UdpFlow(this, key, uid, record)
            udpFlows[key] = flow
            if (!flow.start()) {
                tracker.fromApp(record, uid, ip.length)
                return
            }
        }
        tracker.fromApp(flow.record, flow.uid, ip.length)
        flow.send(ip.data, payloadOffset, payloadLength)
    }

    private fun handleDnsQuery(ip: IpPacket, payloadOffset: Int, payloadLength: Int) {
        val uid = lookupUid(PROTO_UDP, ip)
        tracker.fromApp(null, uid, ip.length)
        val query = ip.data.copyOfRange(payloadOffset, payloadOffset + payloadLength)
        val clientIp = ip.src
        val clientPort = ip.srcPort
        val serverIp = ip.dst
        platform.resolveDns(query) { answer ->
            post { answerDns(uid, query, answer, clientIp, clientPort, serverIp) }
        }
    }

    private fun answerDns(uid: Int, query: ByteArray, answer: ByteArray?, clientIp: Int, clientPort: Int, serverIp: Int) {
        if (!running || query.size < 12) return
        val reply = if (answer != null && answer.size >= 12) {
            answer.copyOf().also { it[0] = query[0]; it[1] = query[1] }
        } else {
            serverFailure(query)
        }
        tracker.dnsAnswer(uid, Dns.parse(reply, 0, reply.size) ?: Dns.parse(query, 0, query.size), failed = answer == null)
        writeToTun(buildUdp(serverIp, 53, clientIp, clientPort, reply, 0, reply.size), null, uid)
    }

    /** The query turned into a SERVFAIL response. */
    private fun serverFailure(query: ByteArray): ByteArray {
        val reply = query.copyOf()
        reply[2] = (reply[2].toInt() or 0x80).toByte() // QR = response
        reply[3] = ((reply[3].toInt() and 0xF0) or 0x80 or 2).toByte() // RA, RCODE = SERVFAIL
        return reply
    }

    internal fun writeToTun(packet: ByteArray, record: ConnectionRecord?, uid: Int) {
        try {
            tun.write(packet, 0, packet.size)
        } catch (e: IOException) {
            // A full TUN queue drops the packet; TCP resends it.
            return
        }
        tracker.toApp(record, uid, packet.size)
        packetTap?.onPacket(packet, 0, packet.size, micros())
    }

    internal fun sendTcp(
        flow: TcpFlow, seq: Int, ack: Int, flags: Int, window: Int,
        payload: ByteArray? = null, payloadLength: Int = payload?.size ?: 0, mss: Int = 0,
    ) {
        val k = flow.key
        val packet = buildTcp(k.dstIp, k.dstPort, k.srcIp, k.srcPort, seq, ack, flags, window, payload, 0, payloadLength, mss)
        writeToTun(packet, flow.record, flow.uid)
    }

    internal fun removeTcp(flow: TcpFlow) {
        if (tcpFlows[flow.key] === flow) tcpFlows.remove(flow.key)
    }

    internal fun removeUdp(flow: UdpFlow) {
        if (udpFlows[flow.key] === flow) udpFlows.remove(flow.key)
    }

    internal companion object {
        val log: Logger = Logger.getLogger("perf-agent")
        const val TIMER_INTERVAL_MS = 100L
        const val MAX_QUEUED_PACKETS = 8192
        const val MAX_TCP_FLOWS = 8192
        const val MAX_UDP_FLOWS = 4096
    }
}
