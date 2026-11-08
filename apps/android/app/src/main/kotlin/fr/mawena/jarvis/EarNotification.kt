package fr.mawena.jarvis

import android.content.Context

/** Notification permanente de l'écoute et alertes (tâche 6). */
object EarNotification {
    const val CHANNEL_LISTEN = "ecoute"
    const val CHANNEL_ALERT = "alerte"
    const val ID_LISTEN = 1
    const val ID_START = 2

    /** Crée les canaux `ecoute` et `alerte` (sans effet s'ils existent). */
    fun createChannels(ctx: Context) {
    }
}
