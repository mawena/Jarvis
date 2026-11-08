package fr.mawena.jarvis

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.provider.Settings
import android.util.Log
import fr.mawena.jarvis.ffi.hasWakeword
import fr.mawena.jarvis.ffi.pairedBrain

/** Démarrage du téléphone ou mise à jour de l'appli : relance de l'écoute (tâche 8). */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED &&
            intent.action != Intent.ACTION_MY_PACKAGE_REPLACED
        ) return
        val dir = context.filesDir.absolutePath
        val ready = try {
            pairedBrain(dir) != null && hasWakeword(dir)
        } catch (e: Throwable) {
            Log.w("Jarvis", "démarrage automatique : état illisible", e)
            false
        }
        val action = BootDecision.decide(
            autostart = Prefs.autostart(context),
            ready = ready,
            overlay = Settings.canDrawOverlays(context),
        )
        Log.i("Jarvis", "démarrage automatique : $action")
        when (action) {
            BootAction.NOTHING -> Unit
            BootAction.LAUNCH_TRAMPOLINE -> context.startActivity(
                Intent(context, StartActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
            )
            BootAction.NOTIFY_TAP_TO_START -> notifyTapToStart(context)
        }
    }

    private fun notifyTapToStart(context: Context) {
        EarNotification.createChannels(context)
        val tap = PendingIntent.getActivity(
            context,
            0,
            Intent(context, StartActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = Notification.Builder(context, EarNotification.CHANNEL_ALERT)
            .setSmallIcon(R.drawable.ic_jarvis)
            .setContentTitle(context.getString(R.string.boot_tap_to_start))
            .setContentText(context.getString(R.string.boot_tap_to_start_text))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java)
            ?.notify(EarNotification.ID_START, notification)
    }
}
