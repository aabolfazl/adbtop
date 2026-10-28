package io.github.aabolfazl.perfagent.engine

import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.Socket

const val UID_UNKNOWN = -1

/** The packet side of the VPN: what apps send comes out of [read], what we [write] goes back to them. */
interface TunDevice {
    /**
     * Reads one IP packet into [buffer]. Returns its length, 0 when nothing arrived within
     * a short wait (so the caller can check whether to stop), or -1 once the device is closed.
     */
    fun read(buffer: ByteArray): Int

    fun write(buffer: ByteArray, offset: Int, length: Int)
}

/** Everything the engine needs from the operating system. */
interface Platform {
    /** Keeps the engine's own sockets out of the VPN so forwarded traffic doesn't loop back. */
    fun protect(socket: Socket): Boolean

    fun protect(socket: DatagramSocket): Boolean

    /** Where to really connect for [destination]; the identity except in tests. */
    fun upstreamAddress(destination: InetSocketAddress): InetSocketAddress = destination

    /** The uid of the app that owns a connection, or [UID_UNKNOWN]. */
    fun ownerUid(protocol: Int, local: InetSocketAddress, remote: InetSocketAddress): Int

    fun appName(uid: Int): String

    /**
     * Resolves a DNS query that an app sent to the VPN's DNS server. [reply] may be called
     * from any thread, with the raw DNS answer or null if resolution failed.
     */
    fun resolveDns(query: ByteArray, reply: (ByteArray?) -> Unit)
}
