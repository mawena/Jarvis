package fr.mawena.jarvis

import android.content.Context
import android.content.SharedPreferences
import android.os.Build

/** Réglages de l'appli (`SharedPreferences` « jarvis »). */
object Prefs {
    private const val FILE = "jarvis"
    private const val NAME = "name"
    private const val THRESHOLD = "threshold"
    private const val AUTOSTART = "autostart"

    /** Seuil du mot d'éveil par défaut (celui de l'oreille du PC), et ses bornes. */
    const val DEFAULT_THRESHOLD = 0.5f
    const val MIN_THRESHOLD = 0.3f
    const val MAX_THRESHOLD = 0.8f

    private fun prefs(ctx: Context): SharedPreferences =
        ctx.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /** Nom de l'oreille annoncé au cerveau ; par défaut le modèle (« Pixel 4 XL »). */
    fun name(ctx: Context): String =
        prefs(ctx).getString(NAME, null)?.takeIf { it.isNotBlank() } ?: Build.MODEL

    fun setName(ctx: Context, name: String) {
        prefs(ctx).edit().putString(NAME, name.trim()).apply()
    }

    /** Seuil du mot d'éveil (0,30 à 0,80), appliqué au prochain démarrage du moteur. */
    fun threshold(ctx: Context): Float =
        prefs(ctx).getFloat(THRESHOLD, DEFAULT_THRESHOLD).coerceIn(MIN_THRESHOLD, MAX_THRESHOLD)

    fun setThreshold(ctx: Context, threshold: Float) {
        prefs(ctx).edit()
            .putFloat(THRESHOLD, threshold.coerceIn(MIN_THRESHOLD, MAX_THRESHOLD))
            .apply()
    }

    /** Démarrer l'écoute avec le téléphone ; vrai par défaut. */
    fun autostart(ctx: Context): Boolean = prefs(ctx).getBoolean(AUTOSTART, true)

    fun setAutostart(ctx: Context, autostart: Boolean) {
        prefs(ctx).edit().putBoolean(AUTOSTART, autostart).apply()
    }
}
