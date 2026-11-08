package fr.mawena.jarvis

/** Ce que fait le récepteur au démarrage du téléphone. */
enum class BootAction { NOTHING, LAUNCH_TRAMPOLINE, NOTIFY_TAP_TO_START }

/** Décision pure (testée sur la JVM) : sans dépendance à Android. */
object BootDecision {
    /**
     * @param autostart réglage « démarrage automatique »
     * @param ready appairé et mot d'éveil présent
     * @param overlay autorisation « Afficher par-dessus d'autres applications »
     */
    fun decide(autostart: Boolean, ready: Boolean, overlay: Boolean): BootAction = when {
        !autostart || !ready -> BootAction.NOTHING
        overlay -> BootAction.LAUNCH_TRAMPOLINE
        else -> BootAction.NOTIFY_TAP_TO_START
    }
}
