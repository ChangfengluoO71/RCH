package com.rch.reader

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.WorkerParameters
import io.flutter.FlutterInjector
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.dart.DartExecutor
import io.flutter.plugin.common.MethodChannel
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

/** A persistent OS wakeup runs the same Dart coordinator as the activity. */
class EhDailyWorker(context: Context, parameters: WorkerParameters) :
    CoroutineWorker(context, parameters) {
    private val mainHandler = Handler(Looper.getMainLooper())
    private var engine: FlutterEngine? = null
    private var workerChannel: MethodChannel? = null

    override suspend fun doWork(): Result {
        try {
            setForeground(createForegroundInfo())
        } catch (_: Exception) {
            recordStatus("system_blocked")
            // No Dart workflow or remote write has started.
            return Result.retry()
        }

        val completed = CompletableDeferred<Unit>()
        try {
            withContext(Dispatchers.Main) {
                val loader = FlutterInjector.instance().flutterLoader()
                loader.startInitialization(applicationContext)
                loader.ensureInitializationComplete(applicationContext, null)
                val current = FlutterEngine(applicationContext)
                engine = current
                RchBackgroundChannels.register(applicationContext, current)
                val channel = MethodChannel(current.dartExecutor.binaryMessenger, "rch/eh_daily_worker")
                workerChannel = channel
                channel.setMethodCallHandler { call, result ->
                    if (call.method != "complete") {
                        result.notImplemented()
                    } else {
                        val status = call.argument<String>("status") ?: "failed"
                        recordStatus(status)
                        result.success(null)
                        channel.setMethodCallHandler(null)
                        workerChannel = null
                        engine = null
                        // Let the platform reply be sent before shutting down
                        // the engine that owns its messenger.
                        mainHandler.post { current.destroy() }
                        completed.complete(Unit)
                    }
                }
                // Cancellation can arrive while the headless engine is still
                // being initialized. The coroutine cancellation handler below
                // may have had no channel to notify, so replay it once ready.
                if (isStopped) channel.invokeMethod("stop", null)
                current.dartExecutor.executeDartEntrypoint(
                    DartExecutor.DartEntrypoint(loader.findAppBundlePath(), "ehDailyBackgroundMain"),
                )
            }
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable + Dispatchers.Main) {
                engine?.destroy()
                engine = null
                workerChannel = null
            }
            throw cancelled
        } catch (_: Exception) {
            recordStatus("initialization_failed")
            withContext(Dispatchers.Main) {
                engine?.destroy()
                engine = null
                workerChannel = null
            }
            return Result.retry()
        }

        // WorkManager's onStopped is final in CoroutineWorker. Observe its
        // coroutine cancellation here, ask Dart to stop at its durable
        // checkpoint, and give it time to finish any in-flight cloud stage.
        try {
            completed.await()
        } catch (cancelled: CancellationException) {
            withContext(NonCancellable) {
                withContext(Dispatchers.Main) {
                    workerChannel?.invokeMethod("stop", null)
                }
                withTimeoutOrNull(10_000L) { completed.await() }
            }
            throw cancelled
        }
        return Result.success()
    }

    private fun recordStatus(status: String) {
        val allowed = setOf(
            "disabled", "waitingForOutputDirectory", "waitingForCurrentRun",
            "waitingForNextDate", "persistenceFailed", "completed", "partial",
            "failed", "cancelled", "interruptedUnknown", "initialization_failed",
            "system_blocked", "running", "claimed", "unsupported", "stopped",
        )
        applicationContext.getSharedPreferences(EhDailyBackgroundScheduler.PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(EhDailyBackgroundScheduler.STATUS, if (status in allowed) status else "failed")
            .putLong(EhDailyBackgroundScheduler.FINISHED_AT, System.currentTimeMillis())
            .apply()
    }

    private fun createForegroundInfo(): ForegroundInfo {
        val channelId = "rch_eh_daily"
        val notificationManager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            notificationManager.createNotificationChannel(
                NotificationChannel(channelId, "EH 每日工作流", NotificationManager.IMPORTANCE_LOW),
            )
        }
        val launchIntent = applicationContext.packageManager.getLaunchIntentForPackage(applicationContext.packageName)
            ?: Intent(applicationContext, MainActivity::class.java)
        val pending = android.app.PendingIntent.getActivity(
            applicationContext, 0, launchIntent,
            android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(applicationContext, channelId)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle("RCH 每日工作流")
            .setContentText("正在检查或处理 EH 订阅")
            .setOngoing(true)
            .setContentIntent(pending)
            .build()
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(7415, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(7415, notification)
        }
    }
}
