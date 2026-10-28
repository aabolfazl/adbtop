package io.github.aabolfazl.perfagent.engine

import com.google.protobuf.ByteString
import io.github.aabolfazl.perfagent.proto.AppTraffic
import io.github.aabolfazl.perfagent.proto.CaptureStats
import io.github.aabolfazl.perfagent.proto.Connection
import io.github.aabolfazl.perfagent.proto.ConnectionState
import io.github.aabolfazl.perfagent.proto.DnsQuery
import io.github.aabolfazl.perfagent.proto.HostSource
import io.github.aabolfazl.perfagent.proto.Protocol
import io.github.aabolfazl.perfagent.proto.Snapshot

internal class ConnectionRecord(
    val id: Long,
    val protocol: Int,
    val uid: Int,
    val localPort: Int,
    val remoteIp: Int,
    val remotePort: Int,
    val startMs: Long,
) {
    var host: String? = null
    var hostSource = HostSource.HOST_NONE
    var rxBytes = 0L
    var txBytes = 0L
    var rxPackets = 0L
    var txPackets = 0L
    var lastMs = startMs
    var state = ConnectionState.CONNECTING
    var error: String? = null

    val isOpen: Boolean get() = state == ConnectionState.CONNECTING || state == ConnectionState.OPEN
}

/**
 * Keeps per-app counters, the connection table, recent DNS lookups and an IP -> name map.
 * Only used from the engine thread.
 */
internal class Tracker(
    private val platform: Platform,
    private val clock: () -> Long,
    private val maxClosed: Int = 300,
    private val maxDns: Int = 200,
) {
    private class AppCounters(val name: String) {
        var rx = 0L
        var tx = 0L
    }

    private val apps = HashMap<Int, AppCounters>()
    private val open = LinkedHashMap<Long, ConnectionRecord>()
    private val closed = ArrayDeque<ConnectionRecord>()
    private val dnsLog = ArrayDeque<DnsQuery>()
    private val hostByIp = object : LinkedHashMap<Int, String>(256, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Int, String>?) = size > 4096
    }
    private var nextId = 1L

    var packetsFromApps = 0L
    var packetsToApps = 0L
    var unsupportedPackets = 0L

    fun open(protocol: Int, uid: Int, localPort: Int, remoteIp: Int, remotePort: Int): ConnectionRecord {
        val record = ConnectionRecord(nextId++, protocol, uid, localPort, remoteIp, remotePort, clock())
        hostByIp[remoteIp]?.let {
            record.host = it
            record.hostSource = HostSource.DNS
        }
        appCounters(uid)
        open[record.id] = record
        return record
    }

    fun close(record: ConnectionRecord, state: ConnectionState, error: String? = null) {
        if (open.remove(record.id) == null) return
        record.state = state
        record.error = error
        closed.addLast(record)
        while (closed.size > maxClosed) closed.removeFirst()
    }

    fun setHost(record: ConnectionRecord, host: String, source: HostSource) {
        record.host = host
        record.hostSource = source
    }

    /** Counts a packet an app sent. */
    fun fromApp(record: ConnectionRecord?, uid: Int, bytes: Int) {
        packetsFromApps++
        appCounters(uid).tx += bytes
        record?.apply {
            txBytes += bytes
            txPackets++
            lastMs = clock()
        }
    }

    /** Counts a packet delivered to an app. */
    fun toApp(record: ConnectionRecord?, uid: Int, bytes: Int) {
        packetsToApps++
        appCounters(uid).rx += bytes
        record?.apply {
            rxBytes += bytes
            rxPackets++
            lastMs = clock()
        }
    }

    fun dnsAnswer(uid: Int, message: DnsMessage?, failed: Boolean) {
        val name = message?.name ?: return
        for (address in message.addresses) {
            if (address.size == 4) hostByIp[u32(address, 0)] = name
        }
        dnsLog.addLast(
            DnsQuery.newBuilder()
                .setTimeMs(clock())
                .setUid(uid)
                .setName(name)
                .addAllAddresses(message.addresses.map { ByteString.copyFrom(it) })
                .setFailed(failed || message.responseCode != 0)
                .build()
        )
        while (dnsLog.size > maxDns) dnsLog.removeFirst()
    }

    fun hostFor(ip: Int): String? = hostByIp[ip]

    private fun appCounters(uid: Int) = apps.getOrPut(uid) {
        AppCounters(if (uid == UID_UNKNOWN) "unknown" else platform.appName(uid))
    }

    fun snapshot(agentVersion: String, startedMs: Long, appFilter: String?, openTcp: Int, openUdp: Int): Snapshot {
        val openByUid = HashMap<Int, Int>()
        for (record in open.values) openByUid[record.uid] = (openByUid[record.uid] ?: 0) + 1

        val builder = Snapshot.newBuilder()
            .setAgentVersion(agentVersion)
            .setTimeMs(clock())
            .setStartedMs(startedMs)
            .setAppFilter(appFilter ?: "")
            .setStats(
                CaptureStats.newBuilder()
                    .setPacketsFromApps(packetsFromApps)
                    .setPacketsToApps(packetsToApps)
                    .setUnsupportedPackets(unsupportedPackets)
                    .setOpenTcp(openTcp)
                    .setOpenUdp(openUdp)
            )
        for ((uid, counters) in apps) {
            builder.addApps(
                AppTraffic.newBuilder()
                    .setUid(uid)
                    .setName(counters.name)
                    .setRxBytes(counters.rx)
                    .setTxBytes(counters.tx)
                    .setOpenConnections(openByUid[uid] ?: 0)
            )
        }
        for (record in closed) builder.addConnections(record.toProto())
        for (record in open.values) builder.addConnections(record.toProto())
        builder.addAllDns(dnsLog)
        return builder.build()
    }

    private fun ConnectionRecord.toProto(): Connection {
        val ip = ByteArray(4)
        put32(ip, 0, remoteIp)
        return Connection.newBuilder()
            .setId(id)
            .setProtocol(if (protocol == PROTO_TCP) Protocol.TCP else Protocol.UDP)
            .setUid(uid)
            .setRemoteIp(ByteString.copyFrom(ip))
            .setRemotePort(remotePort)
            .setLocalPort(localPort)
            .setHost(host ?: "")
            .setHostSource(hostSource)
            .setRxBytes(rxBytes)
            .setTxBytes(txBytes)
            .setRxPackets(rxPackets)
            .setTxPackets(txPackets)
            .setStartMs(startMs)
            .setLastMs(lastMs)
            .setState(state)
            .setError(error ?: "")
            .build()
    }
}
