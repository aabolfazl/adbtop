package io.github.aabolfazl.perfagent

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Log
import io.github.aabolfazl.perfagent.engine.Engine
import io.github.aabolfazl.perfagent.engine.EngineConfig
import java.net.Inet4Address
import java.net.InetAddress

/**
 * A local VPN: Android routes the apps' traffic into it and [Engine] forwards and measures it.
 * Nothing is sent to any VPN server. The system keeps a VPN service bound (and running) while
 * the VPN is up, so no foreground notification is needed.
 */
class CaptureService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private var tunDevice: AndroidTun? = null
    private var engine: Engine? = null
    private var server: ControlServer? = null
    private var appFilter: String? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopCapture()
                stopSelf()
            }
            ACTION_START -> startCapture(intent.getStringExtra(EXTRA_APP)?.takeIf { it.isNotBlank() })
        }
        return START_NOT_STICKY
    }

    private fun startCapture(filter: String?) {
        if (engine != null) {
            if (filter == appFilter) return
            stopCapture()
        }

        val builder = Builder()
            .setSession("Android Perf Agent")
            .setMtu(MTU)
            .addAddress(VPN_ADDRESS, 30)
            .addRoute("0.0.0.0", 0)
            .addDnsServer(DNS_ADDRESS)
            .setBlocking(false)
            // Otherwise Android treats every network as metered while the capture runs.
            .setMetered(false)
        // No IPv6 address or route: Android then blocks IPv6 for captured apps and they use IPv4.
        try {
            if (filter != null) builder.addAllowedApplication(filter) else builder.addDisallowedApplication(packageName)
        } catch (e: PackageManager.NameNotFoundException) {
            Log.e(TAG, "Package $filter is not installed")
            stopSelf()
            return
        }

        val fd = builder.establish()
        if (fd == null) {
            Log.e(TAG, "VPN permission missing; start the capture through ControlActivity")
            stopSelf()
            return
        }

        val device = AndroidTun(fd)
        val newEngine = Engine(
            device,
            AndroidPlatform(this),
            EngineConfig(
                mtu = MTU,
                dnsServer = InetAddress.getByName(DNS_ADDRESS) as Inet4Address,
                appFilter = filter,
                agentVersion = versionName(),
            ),
        )
        val newServer = ControlServer(newEngine)
        tun = fd
        tunDevice = device
        engine = newEngine
        server = newServer
        appFilter = filter
        newServer.start()
        newEngine.start()
        Log.i(TAG, "Capture started" + (filter?.let { " for $it" } ?: " for all apps"))
    }

    private fun stopCapture() {
        server?.close()
        tunDevice?.close()
        engine?.stop()
        runCatching { tun?.close() }
        if (engine != null) Log.i(TAG, "Capture stopped")
        server = null
        tunDevice = null
        engine = null
        tun = null
        appFilter = null
    }

    /** The user turned the VPN off in settings, or another VPN app took over. */
    override fun onRevoke() {
        stopCapture()
        stopSelf()
    }

    override fun onDestroy() {
        stopCapture()
        super.onDestroy()
    }

    private fun versionName(): String =
        runCatching { packageManager.getPackageInfo(packageName, 0).versionName }.getOrNull() ?: "unknown"

    companion object {
        private const val ACTION_START = "io.github.aabolfazl.perfagent.START"
        private const val ACTION_STOP = "io.github.aabolfazl.perfagent.STOP"
        private const val EXTRA_APP = "app"
        private const val MTU = 1500
        private const val VPN_ADDRESS = "10.215.173.1"
        private const val DNS_ADDRESS = "10.215.173.2"

        fun startIntent(context: Context, app: String?): Intent =
            Intent(context, CaptureService::class.java).setAction(ACTION_START).putExtra(EXTRA_APP, app)

        fun stopIntent(context: Context): Intent =
            Intent(context, CaptureService::class.java).setAction(ACTION_STOP)
    }
}
