package io.github.aabolfazl.perfagent.engine

import java.net.InetAddress
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class PacketsTest {
    private val a = ipv4ToInt(InetAddress.getByName("10.0.0.2") as java.net.Inet4Address)
    private val b = ipv4ToInt(InetAddress.getByName("93.184.216.34") as java.net.Inet4Address)

    @Test
    fun tcpPacketRoundTrip() {
        val payload = "hello".toByteArray()
        val packet = buildTcp(a, 40000, b, 443, seq = -5, ack = 123456789, flags = TCP_ACK or TCP_PSH,
            window = 65535, payload = payload, payloadLength = payload.size)

        assertTrue(checksumsValid(packet))
        val ip = IpPacket.parse(packet, packet.size)!!
        assertEquals(PROTO_TCP, ip.protocol)
        assertEquals(a, ip.src)
        assertEquals(b, ip.dst)
        assertEquals(40000, ip.srcPort)
        assertEquals(443, ip.dstPort)

        val tcp = TcpSegment.parse(ip)!!
        assertEquals(-5, tcp.seq)
        assertEquals(123456789, tcp.ack)
        assertTrue(tcp.has(TCP_PSH) && tcp.has(TCP_ACK) && !tcp.has(TCP_SYN))
        assertEquals(65535, tcp.window)
        assertEquals("hello", String(packet, tcp.payloadOffset, tcp.payloadLength))
    }

    @Test
    fun synCarriesMss() {
        val packet = buildTcp(a, 1, b, 2, 0, 0, TCP_SYN or TCP_ACK, 1000, mss = 1460)
        assertTrue(checksumsValid(packet))
        val tcp = TcpSegment.parse(IpPacket.parse(packet, packet.size)!!)!!
        assertEquals(1460, tcp.mss)
        assertEquals(0, tcp.payloadLength)
    }

    @Test
    fun udpPacketRoundTrip() {
        val payload = ByteArray(301) { it.toByte() }
        val packet = buildUdp(a, 5353, b, 53, payload, 0, payload.size)
        assertTrue(checksumsValid(packet))
        val ip = IpPacket.parse(packet, packet.size)!!
        assertEquals(PROTO_UDP, ip.protocol)
        assertEquals(53, ip.dstPort)
        assertEquals(UDP_HEADER + payload.size, u16(packet, ip.l4Offset + 4))
    }

    @Test
    fun fragmentsAndIpv6AreNotParsed() {
        val packet = buildUdp(a, 1, b, 2, ByteArray(10), 0, 10)
        packet[6] = 0x20 // more fragments
        assertNull(IpPacket.parse(packet, packet.size))
        val ipv6 = ByteArray(40).also { it[0] = 0x60 }
        assertNull(IpPacket.parse(ipv6, ipv6.size))
    }

    @Test
    fun corruptedPacketFailsChecksum() {
        val packet = buildTcp(a, 1, b, 2, 1, 2, TCP_ACK, 100, payload = byteArrayOf(1, 2, 3), payloadLength = 3)
        packet[packet.size - 1] = 9
        assertTrue(!checksumsValid(packet))
    }
}

class ByteRingTest {
    @Test
    fun wrapsAround() {
        val ring = ByteRing(8)
        assertEquals(6, ring.write(ByteArray(6) { it.toByte() }, 0, 6))
        ring.skip(4)
        assertEquals(6, ring.write(ByteArray(10) { (10 + it).toByte() }, 0, 10))
        assertEquals(8, ring.size)
        val out = ByteArray(8)
        ring.peek(0, out, 0, 8)
        assertEquals(listOf<Byte>(4, 5, 10, 11, 12, 13, 14, 15), out.toList())
        ring.skip(8)
        assertTrue(ring.isEmpty())
    }
}
