package io.github.aabolfazl.perfagent

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import android.os.Bundle
import android.util.Log

/**
 * Invisible entry point for the terminal UI, which starts and stops the capture with:
 *
 *     adb shell am start -n io.github.aabolfazl.perfagent/.ControlActivity --es cmd start [--es app <package>]
 *     adb shell am start -n io.github.aabolfazl.perfagent/.ControlActivity --es cmd stop
 *
 * The first start shows Android's VPN permission dialog on the phone, which has to be accepted once.
 */
class ControlActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState != null) return // recreated while the consent dialog is open

        when (intent.getStringExtra(EXTRA_COMMAND) ?: COMMAND_START) {
            COMMAND_STOP -> {
                startService(CaptureService.stopIntent(this))
                finish()
            }
            COMMAND_START -> {
                val consent = VpnService.prepare(this)
                if (consent == null) {
                    startCapture()
                } else {
                    @Suppress("DEPRECATION")
                    startActivityForResult(consent, REQUEST_VPN_CONSENT)
                }
            }
            else -> {
                Log.w(TAG, "Unknown command ${intent.getStringExtra(EXTRA_COMMAND)}")
                finish()
            }
        }
    }

    @Deprecated("Framework Activity API")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == REQUEST_VPN_CONSENT) {
            if (resultCode == RESULT_OK) startCapture() else Log.w(TAG, "VPN permission was not granted")
            finish()
        }
    }

    private fun startCapture() {
        startService(CaptureService.startIntent(this, intent.getStringExtra(EXTRA_APP)))
        finish()
    }

    companion object {
        const val EXTRA_COMMAND = "cmd"
        const val EXTRA_APP = "app"
        const val COMMAND_START = "start"
        const val COMMAND_STOP = "stop"
        private const val REQUEST_VPN_CONSENT = 1
    }
}

internal const val TAG = "PerfAgent"
