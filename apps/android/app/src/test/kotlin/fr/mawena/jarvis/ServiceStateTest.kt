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

    @Test
    fun apresCloseAucunDemarrage() {
        val state = ServiceState()
        state.start()
        assertEquals(listOf(ServiceState.Step.STOP_ENGINE), state.close())
        assertEquals(emptyList<ServiceState.Step>(), state.start())
        assertEquals(emptyList<ServiceState.Step>(), state.restart())
        assertEquals(emptyList<ServiceState.Step>(), state.close())
    }

    @Test
    fun arretPerimeIgnoreSiUnDemarrageEstArrive() {
        val ids = StartIds()
        ids.received(1) // ACTION_STOP
        assertTrue(ids.isLatest(1))
        ids.received(2) // ACTION_START juste après
        assertFalse(ids.isLatest(1))
        assertTrue(ids.isLatest(2))
    }
}
