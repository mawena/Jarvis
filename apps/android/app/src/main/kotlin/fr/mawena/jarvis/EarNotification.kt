package fr.mawena.jarvis

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import fr.mawena.jarvis.ffi.EarView
import fr.mawena.jarvis.ffi.MicView

/** Notification permanente de l'écoute et alertes. */
object EarNotification {
    const val CHANNEL_LISTEN = "ecoute"
    const val CHANNEL_ALERT = "alerte"
    const val ID_LISTEN = 1
    const val ID_START = 2

    /** Alerte « configuration à finir » ou erreur de démarrage du moteur. */
    const val ID_SETUP = 3

    /** Crée les canaux `ecoute` et `alerte` (sans effet s'ils existent). */
    fun createChannels(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        val listen = NotificationChannel(
            CHANNEL_LISTEN,
            ctx.getString(R.string.channel_listen),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = ctx.getString(R.string.channel_listen_desc)
            setSound(null, null)
            enableVibration(false)
            setShowBadge(false)
        }
        val alert = NotificationChannel(
            CHANNEL_ALERT,
            ctx.getString(R.string.channel_alert),
            NotificationManager.IMPORTANCE_DEFAULT,
        ).apply {
            description = ctx.getString(R.string.channel_alert_desc)
        }
        nm.createNotificationChannel(listen)
        nm.createNotificationChannel(alert)
    }

    /**
     * Boutons de la notification (pure) : paires (identifiant de chaîne, action du service).
     * Écoute ouverte : « Couper l'écoute » ; coupée : « Réactiver l'écoute » ; toujours
     * « Coupure totale », qui devient « Rouvrir le micro » (seul bouton) en coupure totale.
     */
    fun actionsFor(muted: Boolean, hardMuted: Boolean): List<Pair<Int, String>> {
        if (hardMuted) return listOf(R.string.action_reopen to ListenService.ACTION_REOPEN)
        val listen = if (muted) {
            R.string.action_unmute to ListenService.ACTION_UNMUTE
        } else {
            R.string.action_mute to ListenService.ACTION_MUTE
        }
        return listOf(listen, R.string.action_hard_mute to ListenService.ACTION_HARD_MUTE)
    }

    /** Notification de l'écoute pour [view] (`null` : moteur pas encore prêt). */
    fun build(ctx: Context, view: EarView?): Notification {
        val text = view?.text ?: ctx.getString(R.string.notif_starting)
        val builder = Notification.Builder(ctx, CHANNEL_LISTEN)
            .setSmallIcon(R.drawable.ic_jarvis)
            .setContentTitle(ctx.getString(R.string.notif_title))
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(tapIntent(ctx, view))
        val mic = view?.mic
        val actions = actionsFor(muted = mic == MicView.MUTED, hardMuted = mic == MicView.HARD_MUTED)
        for ((label, action) in actions) {
            val pi = PendingIntent.getService(
                ctx,
                action.hashCode(),
                Intent(ctx, ListenService::class.java).setAction(action),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val icon = Icon.createWithResource(ctx, R.drawable.ic_jarvis)
            builder.addAction(Notification.Action.Builder(icon, ctx.getString(label), pi).build())
        }
        return builder.build()
    }

    /** Alerte (canal `alerte`) qui ouvre l'écran principal : configuration à finir, erreur. */
    fun postSetupAlert(ctx: Context, text: String) {
        val pi = PendingIntent.getActivity(
            ctx,
            0,
            Intent(ctx, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = Notification.Builder(ctx, CHANNEL_ALERT)
            .setSmallIcon(R.drawable.ic_jarvis)
            .setContentTitle(ctx.getString(R.string.notif_title))
            .setContentText(text)
            .setAutoCancel(true)
            .setContentIntent(pi)
            .build()
        ctx.getSystemService(NotificationManager::class.java).notify(ID_SETUP, n)
    }

    /**
     * Toucher : écran principal ; micro bloqué par Android : `StartActivity` avec l'action
     * `ACTION_RESTART` (un toucher de notification rend le micro au service).
     */
    private fun tapIntent(ctx: Context, view: EarView?): PendingIntent {
        val intent = if (view?.mic == MicView.BLOCKED) {
            Intent(ctx, StartActivity::class.java)
                .setAction(ListenService.ACTION_RESTART)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
        } else {
            Intent(ctx, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        }
        return PendingIntent.getActivity(
            ctx,
            1,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }
}
