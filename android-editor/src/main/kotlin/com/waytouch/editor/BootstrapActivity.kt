package com.waytouch.editor

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.util.Log

/**
 * Privileged adb-shell entry point for provisioning the host IPC token.
 *
 * The actual foreground service stays private. ADB shell can launch this
 * activity because it holds android.permission.DUMP; ordinary apps cannot.
 */
class BootstrapActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val token = intent.getStringExtra(EditorService.EXTRA_IPC_TOKEN)
        if (token == null || !TOKEN_PATTERN.matches(token)) {
            Log.e(TAG, "Rejected missing or malformed IPC bootstrap token")
            finish()
            return
        }

        val serviceIntent = Intent(this, EditorService::class.java)
            .setAction(EditorService.ACTION_START)
            .putExtra(EditorService.EXTRA_IPC_TOKEN, token.lowercase())

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(serviceIntent)
            } else {
                startService(serviceIntent)
            }
        } catch (failure: Exception) {
            Log.e(TAG, "Could not start private WayTouch EditorService", failure)
        } finally {
            finish()
        }
    }

    companion object {
        private const val TAG = "WayTouchBootstrap"
        private val TOKEN_PATTERN = Regex("[0-9a-fA-F]{64}")
    }
}
