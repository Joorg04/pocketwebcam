package com.pocketwebcam.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import com.pocketwebcam.app.ui.MainScreen
import com.pocketwebcam.app.ui.PocketTheme

class MainActivity : ComponentActivity() {
    private val model: AppModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // A pairing code scanned with the phone's camera app or Google Lens opens this link.
        if (savedInstanceState == null) handleLink(intent)
        model.onPermission(
            ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED,
        )
        setContent {
            val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
                model.onPermission(it)
            }
            val micLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
                model.onMicPermissionResult()
            }
            // Asked the first time a computer wants the mic, so the reason is obvious.
            LaunchedEffect(model.micPermissionNeeded) {
                if (model.micPermissionNeeded) micLauncher.launch(Manifest.permission.RECORD_AUDIO)
            }
            PocketTheme {
                MainScreen(
                    model, window,
                    requestCamera = { launcher.launch(Manifest.permission.CAMERA) },
                    requestMic = {
                        // After one "Don't allow" Android still shows the dialog once more; after that
                        // only the app's system settings can change it.
                        model.setMicrophone(true)
                        if (shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO)) {
                            micLauncher.launch(Manifest.permission.RECORD_AUDIO)
                        } else {
                            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null)))
                        }
                    },
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleLink(intent)
    }

    private fun handleLink(intent: Intent?) {
        val link = intent?.data?.takeIf { intent.action == Intent.ACTION_VIEW }?.toString() ?: return
        model.handlePairingLink(link)
    }

    override fun onStart() {
        super.onStart()
        model.onVisible(true)
    }

    override fun onStop() {
        model.onVisible(false)
        super.onStop()
    }
}
