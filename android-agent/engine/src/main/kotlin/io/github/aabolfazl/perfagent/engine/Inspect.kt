package io.github.aabolfazl.perfagent.engine

internal class DnsMessage(
    val id: Int,
    val isResponse: Boolean,
    val responseCode: Int,
    /** Name of the first question. */
    val name: String?,
    /** A and AAAA records in the answer section (4 or 16 bytes each). */
    val addresses: List<ByteArray>,
)

internal object Dns {
    private const val TYPE_A = 1
    private const val TYPE_AAAA = 28

    fun parse(data: ByteArray, offset: Int, length: Int): DnsMessage? {
        if (length < 12) return null
        val end = offset + length
        val flags = u16(data, offset + 2)
        val questions = u16(data, offset + 4)
        val answers = u16(data, offset + 6)

        var pos = offset + 12
        var name: String? = null
        repeat(questions) { index ->
            val (qname, next) = readName(data, offset, end, pos) ?: return null
            if (index == 0) name = qname
            pos = next + 4 // type + class
            if (pos > end) return null
        }

        val addresses = ArrayList<ByteArray>()
        repeat(answers) {
            val (_, next) = readName(data, offset, end, pos) ?: return@repeat
            if (next + 10 > end) return@repeat
            val type = u16(data, next)
            val rdLength = u16(data, next + 8)
            val rdata = next + 10
            if (rdata + rdLength > end) return@repeat
            if ((type == TYPE_A && rdLength == 4) || (type == TYPE_AAAA && rdLength == 16)) {
                addresses.add(data.copyOfRange(rdata, rdata + rdLength))
            }
            pos = rdata + rdLength
        }

        return DnsMessage(
            id = u16(data, offset),
            isResponse = flags and 0x8000 != 0,
            responseCode = flags and 0x0F,
            name = name,
            addresses = addresses,
        )
    }

    /** Reads a possibly compressed name; returns it and the position right after it. */
    private fun readName(data: ByteArray, base: Int, end: Int, start: Int): Pair<String, Int>? {
        val labels = StringBuilder()
        var pos = start
        var after = -1
        var jumps = 0
        while (true) {
            if (pos >= end) return null
            val len = data[pos].toInt() and 0xFF
            when {
                len == 0 -> {
                    if (after < 0) after = pos + 1
                    return labels.toString() to after
                }
                len and 0xC0 == 0xC0 -> {
                    if (pos + 1 >= end || ++jumps > 16) return null
                    if (after < 0) after = pos + 2
                    pos = base + (((len and 0x3F) shl 8) or (data[pos + 1].toInt() and 0xFF))
                }
                else -> {
                    if (pos + 1 + len > end || labels.length > 255) return null
                    if (labels.isNotEmpty()) labels.append('.')
                    for (i in pos + 1..pos + len) labels.append((data[i].toInt() and 0xFF).toChar())
                    pos += 1 + len
                }
            }
        }
    }
}

internal sealed class HostResult {
    class Found(val host: String, val source: Int) : HostResult()
    object NeedMore : HostResult()
    object NotFound : HostResult()
}

/** Finds the server name in the first bytes an app sends on a TCP connection. */
internal object HostSniffer {
    const val SOURCE_TLS = 2
    const val SOURCE_HTTP = 3

    /** How much of the start of a connection is kept for sniffing. */
    const val MAX_BYTES = 16 * 1024

    private val httpMethods = listOf("GET ", "POST ", "HEAD ", "PUT ", "DELETE ", "OPTIONS ", "PATCH ", "CONNECT ")

    fun sniff(data: ByteArray, length: Int): HostResult {
        if (length == 0) return HostResult.NeedMore
        if (data[0].toInt() == 0x16) return tlsServerName(data, length)
        return httpHost(data, length)
    }

    private fun tlsServerName(data: ByteArray, length: Int): HostResult {
        if (length < 5) return HostResult.NeedMore
        if (data[1].toInt() != 0x03) return HostResult.NotFound
        val recordEnd = 5 + u16(data, 3)
        if (recordEnd > MAX_BYTES) return HostResult.NotFound
        if (length < recordEnd) return HostResult.NeedMore

        var pos = 5
        if (data[pos].toInt() != 0x01) return HostResult.NotFound // ClientHello
        pos += 4 // type + 24-bit length
        pos += 2 + 32 // version + random
        if (pos >= recordEnd) return HostResult.NotFound
        pos += 1 + (data[pos].toInt() and 0xFF) // session id
        if (pos + 2 > recordEnd) return HostResult.NotFound
        pos += 2 + u16(data, pos) // cipher suites
        if (pos + 1 > recordEnd) return HostResult.NotFound
        pos += 1 + (data[pos].toInt() and 0xFF) // compression methods
        if (pos + 2 > recordEnd) return HostResult.NotFound
        val extensionsEnd = minOf(recordEnd, pos + 2 + u16(data, pos))
        pos += 2

        while (pos + 4 <= extensionsEnd) {
            val type = u16(data, pos)
            val extLength = u16(data, pos + 2)
            pos += 4
            if (pos + extLength > extensionsEnd) break
            if (type == 0 && extLength >= 5) {
                // server_name: list length, name type (0 = host name), name length, name
                val nameLength = u16(data, pos + 3)
                if (data[pos + 2].toInt() == 0 && pos + 5 + nameLength <= extensionsEnd) {
                    return HostResult.Found(String(data, pos + 5, nameLength, Charsets.US_ASCII), SOURCE_TLS)
                }
            }
            pos += extLength
        }
        return HostResult.NotFound
    }

    private fun httpHost(data: ByteArray, length: Int): HostResult {
        val start = String(data, 0, minOf(length, 8), Charsets.US_ASCII)
        val isHttp = httpMethods.any { it.startsWith(start) || start.startsWith(it) }
        if (!isHttp) return HostResult.NotFound

        val text = String(data, 0, minOf(length, 8192), Charsets.ISO_8859_1)
        val headersEnd = text.indexOf("\r\n\r\n")
        val lines = (if (headersEnd >= 0) text.substring(0, headersEnd) else text).split("\r\n")
        // Without the end of the headers, the last line may still be cut off.
        val completeLines = if (headersEnd >= 0) lines else lines.dropLast(1)
        for (line in completeLines.drop(1)) {
            if (line.length > 5 && line.regionMatches(0, "host:", 0, 5, ignoreCase = true)) {
                val host = line.substring(5).trim().substringBefore(':')
                if (host.isNotEmpty()) return HostResult.Found(host, SOURCE_HTTP)
            }
        }
        return if (headersEnd >= 0 || length >= 8192) HostResult.NotFound else HostResult.NeedMore
    }
}
