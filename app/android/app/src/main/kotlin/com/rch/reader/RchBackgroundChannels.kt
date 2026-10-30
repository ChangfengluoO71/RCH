package com.rch.reader

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Environment
import androidx.core.content.ContextCompat
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/** Non-interactive channels needed by both activity and headless engines. */
object RchBackgroundChannels {
    fun register(context: Context, engine: FlutterEngine) {
        val app = context.applicationContext
        MethodChannel(engine.dartExecutor.binaryMessenger, "rch/storage")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "nativeLibraryDir" -> result.success(app.applicationInfo.nativeLibraryDir)
                    "isAllFilesAccessGranted" -> result.success(
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                            Environment.isExternalStorageManager()
                        } else {
                            ContextCompat.checkSelfPermission(
                                app, android.Manifest.permission.READ_EXTERNAL_STORAGE,
                            ) == PackageManager.PERMISSION_GRANTED
                        },
                    )
                    else -> result.notImplemented()
                }
            }
        val vault = CredentialVault(app)
        MethodChannel(engine.dartExecutor.binaryMessenger, "rch/credentials")
            .setMethodCallHandler { call, result ->
                val key = call.argument<String>("key")
                if (key.isNullOrBlank()) {
                    result.error("bad_args", "credential key is required", null)
                    return@setMethodCallHandler
                }
                runCatching {
                    when (call.method) {
                        "put" -> {
                            val value = call.argument<String>("value")
                            require(value != null) { "credential value is required" }
                            vault.put(key, value)
                            result.success(null)
                        }
                        "get" -> result.success(vault.get(key))
                        "delete" -> {
                            vault.delete(key)
                            result.success(null)
                        }
                        else -> result.notImplemented()
                    }
                }.onFailure {
                    // Never return credential content, ciphertext or paths.
                    result.error("vault_error", "credential vault operation failed", null)
                }
            }
        EhDailyBackgroundScheduler.register(app, engine)
    }
}
