package fr.mawena.jarvis

import fr.mawena.jarvis.ServiceState.Step
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ServiceStateTest {
    @Test
    fun startIgnoreSiDejaLance() {
        val s = ServiceState()
        assertEquals(listOf(Step.START_ENGINE), s.start())
        assertTrue(s.running)
        assertEquals(emptyList<Step>(), s.start())
        assertEquals(emptyList<Step>(), s.start())
    }

    @Test
    fun restartArreteAvantDeRelancer() {
        val s = ServiceState()
        s.start()
        assertEquals(listOf(Step.STOP_ENGINE, Step.START_ENGINE), s.restart())
        assertTrue(s.running)
    }

    @Test
    fun restartSansMoteurSeContenteDeDemarrer() {
        assertEquals(listOf(Step.START_ENGINE), ServiceState().restart())
    }

    @Test
    fun stopIdempotent() {
        val s = ServiceState()
        s.start()
        assertEquals(listOf(Step.STOP_ENGINE), s.stop())
        assertFalse(s.running)
        assertEquals(emptyList<Step>(), s.stop())
    }

    @Test
    fun echecDeDemarragePermetUnNouvelEssai() {
        val s = ServiceState()
        s.start()
        s.startFailed()
        assertEquals(listOf(Step.START_ENGINE), s.start())
    }
}
