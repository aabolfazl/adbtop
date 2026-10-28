package io.github.aabolfazl.perfagent.engine

import java.net.Inet4Address
import java.net.InetAddress

internal const val PROTO_TCP = 6
internal const val PROTO_UDP = 17

internal const val TCP_FIN = 0x01
internal const val TCP_SYN = 0x02
internal const val TCP_RST = 0x04
internal const val TCP_PSH = 0x08
internal const val TCP_ACK = 0x10

internal const val IPV4_HEADER = 20
internal const val TCP_HEADER = 20
internal const val UDP_HEADER = 8

internal fun u16(data: ByteArray, offset: Int): Int =
    ((data[offset].toInt() and 0xFF) shl 8) or (data[offset + 1].toInt() and 0xFF)

internal fun u32(data: ByteArray, offset: Int): Int =
    (u16(data, offset) shl 16) or u16(data, offset + 2)

internal fun put16(data: ByteArray, offset: Int, value: Int) {
    data[offset] = (value ushr 8).toByte()
    data[offset + 1] = value.toByte()
}

internal fun put32(data: ByteArray, offset: Int, value: Int) {
    put16(data, offset, value ushr 16)
    put16(data, offset + 2, value)
}

internal fun ipv4ToInt(address: Inet4Address): Int = u32(address.address, 0)

internal fun intToIpv4(value: Int): Inet4Address {
    val bytes = ByteArray(4)
    put32(bytes, 0, value)
    return InetAddress.getByAddress(bytes) as Inet4Address
}

internal fun ipv4ToString(value: Int): String =
    "${value ushr 24}.${(value ushr 16) and 0xFF}.${(value ushr 8) and 0xFF}.${value and 0xFF}"

/** An IPv4 packet with a TCP or UDP payload, parsed in place. */
internal class IpPacket(
    val data: ByteArray,
    val length: Int,
    val protocol: Int,
    val src: Int,
    val dst: Int,
    /** Offset of the TCP/UDP header. */
    val l4Offset: Int,
) {
    val srcPort: Int get() = u16(data, l4Offset)
    val dstPort: Int get() = u16(data, l4Offset + 2)

    companion object {
        /** Returns null for anything that isn't an unfragmented IPv4 packet. */
        fun parse(data: ByteArray, length: Int): IpPacket? {
            if (length < IPV4_HEADER) return null
            if ((data[0].toInt() ushr 4) != 4) return null
            val headerLength = (data[0].toInt() and 0x0F) * 4
            val totalLength = u16(data, 2)
            if (headerLength < IPV4_HEADER || totalLength < headerLength || totalLength > length) return null
            // More-fragments flag or a fragment offset: fragments are not reassembled.
            if (u16(data, 6) and 0x3FFF != 0) return null
            return IpPacket(
                data = data,
                length = totalLength,
                protocol = data[9].toInt() and 0xFF,
                src = u32(data, 12),
                dst = u32(data, 16),
                l4Offset = headerLength,
            )
        }
    }
}

internal class TcpSegment(
    val seq: Int,
    val ack: Int,
    val flags: Int,
    val window: Int,
    /** Maximum segment size option from a SYN, or 0 if absent. */
    val mss: Int,
    val payloadOffset: Int,
    val payloadLength: Int,
) {
    fun has(flag: Int) = flags and flag != 0

    companion object {
        fun parse(ip: IpPacket): TcpSegment? {
            val o = ip.l4Offset
            if (ip.length - o < TCP_HEADER) return null
            val headerLength = ((ip.data[o + 12].toInt() ushr 4) and 0x0F) * 4
            if (headerLength < TCP_HEADER || o + headerLength > ip.length) return null
            return TcpSegment(
                seq = u32(ip.data, o + 4),
                ack = u32(ip.data, o + 8),
                flags = ip.data[o + 13].toInt() and 0x3F,
                window = u16(ip.data, o + 14),
                mss = parseMss(ip.data, o + TCP_HEADER, o + headerLength),
                payloadOffset = o + headerLength,
                payloadLength = ip.length - o - headerLength,
            )
        }

        private fun parseMss(data: ByteArray, start: Int, end: Int): Int {
            var i = start
            while (i < end) {
                when (val kind = data[i].toInt() and 0xFF) {
                    0 -> return 0
                    1 -> i++
                    else -> {
                        if (i + 1 >= end) return 0
                        val len = data[i + 1].toInt() and 0xFF
                        if (len < 2 || i + len > end) return 0
                        if (kind == 2 && len == 4) return u16(data, i + 2)
                        i += len
                    }
                }
            }
            return 0
        }
    }
}

/** Ones' complement sum used by the IP, TCP and UDP checksums. */
private fun sum16(data: ByteArray, offset: Int, length: Int, initial: Long = 0): Long {
    var sum = initial
    var i = offset
    val end = offset + length
    while (i + 1 < end) {
        sum += u16(data, i)
        i += 2
    }
    if (i < end) sum += (data[i].toInt() and 0xFF) shl 8
    return sum
}

private fun fold(sum: Long): Int {
    var s = sum
    while (s ushr 16 != 0L) s = (s and 0xFFFF) + (s ushr 16)
    return s.toInt().inv() and 0xFFFF
}

private val nextIpId = java.util.concurrent.atomic.AtomicInteger()

private fun writeIpv4Header(packet: ByteArray, totalLength: Int, protocol: Int, src: Int, dst: Int) {
    packet[0] = 0x45
    packet[1] = 0
    put16(packet, 2, totalLength)
    put16(packet, 4, nextIpId.incrementAndGet())
    put16(packet, 6, 0x4000) // don't fragment
    packet[8] = 64
    packet[9] = protocol.toByte()
    put16(packet, 10, 0)
    put32(packet, 12, src)
    put32(packet, 16, dst)
    put16(packet, 10, fold(sum16(packet, 0, IPV4_HEADER)))
}

private fun transportChecksum(packet: ByteArray, protocol: Int, src: Int, dst: Int, l4Offset: Int, l4Length: Int): Int {
    var pseudo = 0L
    pseudo += (src ushr 16) + (src and 0xFFFF)
    pseudo += (dst ushr 16) + (dst and 0xFFFF)
    pseudo += protocol
    pseudo += l4Length
    return fold(sum16(packet, l4Offset, l4Length, pseudo))
}

/** Builds an IPv4/TCP packet. A SYN gets an MSS option when [mss] is above 0. */
internal fun buildTcp(
    src: Int, srcPort: Int, dst: Int, dstPort: Int,
    seq: Int, ack: Int, flags: Int, window: Int,
    payload: ByteArray? = null, payloadOffset: Int = 0, payloadLength: Int = 0,
    mss: Int = 0,
): ByteArray {
    val optionsLength = if (mss > 0) 4 else 0
    val tcpLength = TCP_HEADER + optionsLength + payloadLength
    val packet = ByteArray(IPV4_HEADER + tcpLength)
    writeIpv4Header(packet, packet.size, PROTO_TCP, src, dst)

    val o = IPV4_HEADER
    put16(packet, o, srcPort)
    put16(packet, o + 2, dstPort)
    put32(packet, o + 4, seq)
    put32(packet, o + 8, ack)
    packet[o + 12] = (((TCP_HEADER + optionsLength) / 4) shl 4).toByte()
    packet[o + 13] = flags.toByte()
    put16(packet, o + 14, window)
    if (mss > 0) {
        packet[o + 20] = 2
        packet[o + 21] = 4
        put16(packet, o + 22, mss)
    }
    if (payload != null && payloadLength > 0) {
        System.arraycopy(payload, payloadOffset, packet, o + TCP_HEADER + optionsLength, payloadLength)
    }
    put16(packet, o + 16, transportChecksum(packet, PROTO_TCP, src, dst, IPV4_HEADER, tcpLength))
    return packet
}

internal fun buildUdp(
    src: Int, srcPort: Int, dst: Int, dstPort: Int,
    payload: ByteArray, payloadOffset: Int, payloadLength: Int,
): ByteArray {
    val udpLength = UDP_HEADER + payloadLength
    val packet = ByteArray(IPV4_HEADER + udpLength)
    writeIpv4Header(packet, packet.size, PROTO_UDP, src, dst)

    val o = IPV4_HEADER
    put16(packet, o, srcPort)
    put16(packet, o + 2, dstPort)
    put16(packet, o + 4, udpLength)
    System.arraycopy(payload, payloadOffset, packet, o + UDP_HEADER, payloadLength)
    // A computed checksum of 0 is sent as 0xFFFF; 0 means "no checksum" in UDP.
    val checksum = transportChecksum(packet, PROTO_UDP, src, dst, IPV4_HEADER, udpLength)
    put16(packet, o + 6, if (checksum == 0) 0xFFFF else checksum)
    return packet
}

/** True when the IPv4 header checksum and the TCP/UDP checksum are both valid. */
internal fun checksumsValid(packet: ByteArray): Boolean {
    val ip = IpPacket.parse(packet, packet.size) ?: return false
    if (fold(sum16(packet, 0, ip.l4Offset)) != 0) return false
    return transportChecksum(packet, ip.protocol, ip.src, ip.dst, ip.l4Offset, ip.length - ip.l4Offset) == 0
}
