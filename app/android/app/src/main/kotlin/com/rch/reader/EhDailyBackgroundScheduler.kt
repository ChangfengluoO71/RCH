package com.rch.reader

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.core.content.ContextCompat
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.TimeUnit

object EhDailyBackgroundScheduler {
    private const val UNIQUE_WORK = "rch_eh_daily_v1"
    const val PREFS = "rch_eh_daily_status"
    const val STATUS = "last_status"
    const val FINISHED_AT = "last_finished_at"

    fun register(context: Context, engine: FlutterEngine) {
        val manager = WorkManager.getInstance(context)
        val mainExecutor = ContextCompat.getMainExecutor(context)
        MethodChannel(engine.dartExecutor.binaryMessenger, "rch/eh_daily_background")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "setEnabled" -> {
                        val enabled = call.argument<Boolean>("enabled")
                        if (enabled == null) {
                            result.error("bad_args", "enabled is required", null)
                            return@setMethodCallHandler
                        }
                        val operation = if (enabled) {
                            val request = PeriodicWorkRequestBuilder<EhDailyWorker>(6, TimeUnit.HOURS)
                                .setConstraints(
                                    Constraints.Builder()
                                        .setRequiredNetworkType(NetworkType.CONNECTED)
                                        .build(),
                                )
                                .build()
                            manager.enqueueUniquePeriodicWork(
                                UNIQUE_WORK, ExistingPeriodicWorkPolicy.UPDATE, request,
                            )
                        } else {
                            manager.cancelUniqueWork(UNIQUE_WORK)
                        }
                        operation.result.addListener({
                            runCatching { operation.result.get() }
                                .onSuccess { result.success(null) }
                                .onFailure { result.error("schedule_failed", "unable to update background schedule", null) }
                        }, mainExecutor)
                    }
                    "status" -> {
                        val infos = manager.getWorkInfosForUniqueWork(UNIQUE_WORK)
                        infos.addListener({
                            runCatching {
                                val rows = infos.get()
                                val connectivity = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
                                val capabilities = connectivity.getNetworkCapabilities(connectivity.activeNetwork)
                                val connected = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
                                val preferences = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                                mapOf(
                                    "scheduled" to rows.any { !it.state.isFinished },
                                    "running" to rows.any { it.state == WorkInfo.State.RUNNING },
                                    "networkConnected" to connected,
                                    "lastStatus" to preferences.getString(STATUS, null),
                                    "lastFinishedAt" to preferences.getLong(FINISHED_AT, 0L),
                                )
                            }.onSuccess { result.success(it) }
                                .onFailure { result.error("status_failed", "unable to read background status", null) }
                        }, mainExecutor)
                    }
                    else -> result.notImplemented()
                }
            }
    }
}
