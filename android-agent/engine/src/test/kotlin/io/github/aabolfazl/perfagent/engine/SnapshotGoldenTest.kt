package io.github.aabolfazl.perfagent.engine

import io.github.aabolfazl.perfagent.proto.ConnectionState
import io.github.aabolfazl.perfagent.proto.HostSource
import io.github.aabolfazl.perfagent.proto.Snapshot
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Writes proto/testdata/snapshots.bin: five snapshots as the agent sends them (length-delimited),
 * one second apart. The Rust side decodes the same file in its tests, which checks that both ends
 * agree on the wire format. Regenerate with UPDATE_GOLDEN=1 after changing proto/agent.proto.
 */
class SnapshotGoldenTest {
    private val file = File("../../proto/testdata/snapshots.bin")

    private fun snapshots(): List<Snapshot> {
        var now = 1_700_000_000_000L
        val platform = object : Platform {
            override fun protect(socket: Socket) = true
            override fun protect(socket: DatagramSocket) = true
            override fun ownerUid(protocol: Int, local: InetSocketAddress, remote: InetSocketAddress) = UID_UNKNOWN
            override fun appName(uid: Int) = if (uid == 10123) "com.example.app" else "com.other.app"
            override fun resolveDns(query: ByteArray, reply: (ByteArray?) -> Unit) = reply(null)
        }
        val tracker = Tracker(platform, { now })
        val started = now

        val query = dnsQuery(1, "www.example.com")
        val answer = dnsAnswer(query, byteArrayOf(93.toByte(), 184.toByte(), 216.toByte(), 34))
        tracker.dnsAnswer(10123, Dns.parse(answer, 0, answer.size), failed = false)

        val ip = { a: Int, b: Int, c: Int, d: Int -> (a shl 24) or (b shl 16) or (c shl 8) or d }
        val web = tracker.open(PROTO_TCP, 10123, 40001, ip(93, 184, 216, 34), 443)
        web.state = ConnectionState.OPEN
        tracker.setHost(web, "www.example.com", HostSource.TLS_SNI)
        val api = tracker.open(PROTO_TCP, 10123, 40002, ip(10, 1, 2, 3), 80)
        api.state = ConnectionState.OPEN
        tracker.setHost(api, "api.example.com", HostSource.HTTP_HOST)
        val other = tracker.open(PROTO_UDP, 10200, 50000, ip(8, 8, 8, 8), 53)
        other.state = ConnectionState.OPEN

        return (1..5).map { second ->
            now += 1000
            repeat(10) { tracker.toApp(web, 10123, 1500) } // 15 kB/s down
            tracker.fromApp(web, 10123, 500)
            tracker.toApp(other, 10200, 100)
            if (second == 3) tracker.close(api, ConnectionState.CLOSED)
            tracker.snapshot("golden", started, null, openTcp = 1, openUdp = 1)
        }
    }

    @Test
    fun goldenSnapshotsMatchTheCheckedInFile() {
        val expected = snapshots()
        val bytes = ByteArrayOutputStream().also { out -> expected.forEach { it.writeDelimitedTo(out) } }.toByteArray()
        if (System.getenv("UPDATE_GOLDEN") != null || !file.exists()) {
            file.parentFile.mkdirs()
            file.writeBytes(bytes)
        }
        val input = file.inputStream()
        val decoded = generateSequence { Snapshot.parseDelimitedFrom(input) }.toList()
        assertEquals(expected, decoded)
        assertEquals(15_000L * 5, decoded.last().appsList.first { it.uid == 10123 }.rxBytes)
    }
}
