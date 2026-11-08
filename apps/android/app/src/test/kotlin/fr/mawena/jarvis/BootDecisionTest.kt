package fr.mawena.jarvis

import org.junit.Assert.assertEquals
import org.junit.Test

class BootDecisionTest {
    @Test
    fun desactiveNeFaitRien() {
        assertEquals(BootAction.NOTHING, BootDecision.decide(autostart = false, ready = true, overlay = true))
        assertEquals(BootAction.NOTHING, BootDecision.decide(autostart = false, ready = true, overlay = false))
    }

    @Test
    fun pasPretNeFaitRien() {
        assertEquals(BootAction.NOTHING, BootDecision.decide(autostart = true, ready = false, overlay = true))
        assertEquals(BootAction.NOTHING, BootDecision.decide(autostart = true, ready = false, overlay = false))
    }

    @Test
    fun avecSuperpositionLanceLaTrampoline() {
        assertEquals(BootAction.LAUNCH_TRAMPOLINE, BootDecision.decide(autostart = true, ready = true, overlay = true))
    }

    @Test
    fun sansSuperpositionNotificationATapper() {
        assertEquals(BootAction.NOTIFY_TAP_TO_START, BootDecision.decide(autostart = true, ready = true, overlay = false))
    }
}
