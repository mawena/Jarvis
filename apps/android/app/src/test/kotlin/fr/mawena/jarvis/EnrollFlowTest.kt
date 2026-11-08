package fr.mawena.jarvis

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class EnrollFlowTest {
    @Test
    fun huitPrisesPuisFini() {
        val flow = EnrollFlow()
        for (n in 1..8) {
            assertFalse(flow.done)
            assertEquals(n, flow.take)
            flow.accepted()
        }
        assertTrue(flow.done)
    }

    @Test
    fun priseTropFaibleRefaiteSousLeMemeNumero() {
        val flow = EnrollFlow()
        flow.accepted()
        flow.accepted()
        assertEquals(3, flow.take)
        flow.tooQuiet()
        assertEquals(3, flow.take)
        assertFalse(flow.done)
    }
}
