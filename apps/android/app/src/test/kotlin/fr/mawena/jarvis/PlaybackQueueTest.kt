package fr.mawena.jarvis

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaybackQueueTest {
    private val pcm = ByteArray(4)

    @Test
    fun idleSeulementPourLaDerniereGeneration() {
        val q = PlaybackQueue()
        q.put(pcm, 16000, 1)
        q.put(pcm, 16000, 2)
        assertEquals(1L, q.poll()!!.generation)
        // Il reste le son 2 : la fin du son 1 ne se signale pas.
        assertNull(q.drained())
        assertEquals(2L, q.poll()!!.generation)
        assertEquals(2L, q.drained())
        // Déjà signalée : pas de doublon.
        assertNull(q.drained())
    }

    @Test
    fun nouveauSonApresUneFinEstSignaleAPart() {
        val q = PlaybackQueue()
        q.put(pcm, 16000, 1)
        q.poll()
        assertEquals(1L, q.drained())
        q.put(pcm, 16000, 2)
        q.poll()
        assertEquals(2L, q.drained())
    }

    @Test
    fun rienAJouerRienASignaler() {
        assertNull(PlaybackQueue().drained())
    }

    @Test
    fun stopVideLaFile() {
        val q = PlaybackQueue()
        q.put(pcm, 16000, 1)
        q.put(pcm, 16000, 2)
        val epoch = q.currentEpoch
        q.stop()
        assertNull(q.poll())
        // Pas d'appel à playbackIdle après un arrêt demandé par le moteur.
        assertNull(q.drained())
        assertTrue(q.currentEpoch > epoch)
        // La file reste utilisable.
        q.put(pcm, 16000, 3)
        assertEquals(3L, q.poll()!!.generation)
        assertEquals(3L, q.drained())
    }

    @Test
    fun changementDeFrequenceRecreeLaSortie() {
        val q = PlaybackQueue()
        assertTrue(q.needsNewOutput(22050))
        assertFalse(q.needsNewOutput(22050))
        assertTrue(q.needsNewOutput(48000))
        assertFalse(q.needsNewOutput(48000))
        q.outputReleased()
        assertTrue(q.needsNewOutput(48000))
    }

    @Test
    fun takeRendNullApresLeDelai() {
        assertNull(PlaybackQueue().take(20))
    }

    @Test
    fun closeLibereLesAttentes() {
        val q = PlaybackQueue()
        q.close()
        q.put(pcm, 16000, 1)
        assertNull(q.take(1000))
    }

    @Test
    fun sonPrisAvantUnArretEstPerime() {
        val q = PlaybackQueue()
        q.put(pcm, 16000, 1)
        val chunk = q.take(100)!!
        assertEquals(q.currentEpoch, chunk.epoch)
        // Arrêt entre la prise du son et sa lecture : le lecteur voit l'époque changer.
        q.stop()
        assertTrue(chunk.epoch != q.currentEpoch)
        // Un son entré après l'arrêt porte la nouvelle époque.
        q.put(pcm, 16000, 2)
        assertEquals(q.currentEpoch, q.take(100)!!.epoch)
    }
}
