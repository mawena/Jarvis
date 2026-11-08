package fr.mawena.jarvis

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import fr.mawena.jarvis.ffi.EarEngine
import fr.mawena.jarvis.ffi.EarException
import fr.mawena.jarvis.ffi.EarHost
import fr.mawena.jarvis.ffi.EarSettings
import fr.mawena.jarvis.ffi.EarView
import java.util.concurrent.Executors

/**
 * Service d'écoute au premier plan, type micro.
 *
 * Fils : le principal (cycle de vie, notification), « jarvis-moteur » (un seul fil, dans
 * l'ordre : création, démarrage, arrêt du moteur, coupures), « jarvis-micro » et
 * « jarvis-voix » (voir [AudioIn], [AudioOut]). Les rappels de l'hôte arrivent d'un fil
 * Rust et ne font que mettre en file ou poster : jamais d'attente.
 */
class ListenService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "jarvis-moteur") }

    /** Accédé seulement depuis `worker`. */
    private val state = ServiceState()

    @Volatile private var engine: EarEngine? = null
    private lateinit var audioIn: AudioIn
    private lateinit var audioOut: AudioOut

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null

    /** Pont vers Kotlin : aucune méthode ne bloque. */
    private val host = object : EarHost {
        override fun startCapture() = audioIn.start()
        override fun stopCapture() = audioIn.stop()
        override fun playPcm(pcmLe: ByteArray, sampleRate: UInt, generation: ULong) =
            audioOut.play(pcmLe, sampleRate.toInt(), generation.toLong())
        override fun stopPlayback() = audioOut.stop()
        override fun statusChanged(view: EarView) {
            lastView = view
            main.post { notify(view) }
        }
    }

    override fun onCreate() {
        super.onCreate()
        alive = true
        EarNotification.createChannels(this)
        audioIn = AudioIn { engine }
        audioOut = AudioOut(this) { engine }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Dans les cinq secondes après startForegroundService, quelle que soit l'action.
        try {
            startForeground(
                EarNotification.ID_LISTEN,
                EarNotification.build(this, lastView),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } catch (ex: RuntimeException) {
            // Micro refusé (SecurityException) ou démarrage interdit depuis l'arrière-plan.
            Log.w(TAG, "premier plan refusé", ex)
            EarNotification.postSetupAlert(this, getString(R.string.notif_setup_needed))
            stopSelf()
            return START_NOT_STICKY
        }
        // Redémarrage « sticky » (intent nul) = démarrage.
        when (val action = intent?.action ?: ACTION_START) {
            ACTION_START -> worker.execute { apply(state.start()) }
            ACTION_RESTART -> worker.execute { apply(state.restart()) }
            ACTION_STOP -> worker.execute {
                apply(state.stop())
                main.post {
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                }
            }
            ACTION_MUTE -> worker.execute { engine?.mute() }
            ACTION_UNMUTE -> worker.execute { engine?.unmute() }
            ACTION_HARD_MUTE -> worker.execute { engine?.hardMute() }
            ACTION_REOPEN -> worker.execute { engine?.reopen() }
            else -> Log.w(TAG, "action inconnue : $action")
        }
        return START_STICKY
    }

    override fun onDestroy() {
        // Arrêt hors du fil principal : `stop()` du moteur peut attendre jusqu'à 5 s.
        val teardown = Thread({
            worker.submit { apply(state.stop()) }.get()
            worker.shutdown()
            audioIn.stop()
            audioOut.release()
        }, "jarvis-arret")
        releaseLocks()
        current = null
        lastView = null
        alive = false
        teardown.start()
        super.onDestroy()
    }

    /** Exécute les étapes dans l'ordre, sur le fil du moteur. */
    private fun apply(steps: List<ServiceState.Step>) {
        for (step in steps) {
            when (step) {
                ServiceState.Step.STOP_ENGINE -> stopEngine()
                ServiceState.Step.START_ENGINE -> if (!startEngine()) {
                    state.startFailed()
                    main.post {
                        stopForeground(STOP_FOREGROUND_REMOVE)
                        stopSelf()
                    }
                    return
                }
            }
        }
    }

    private fun startEngine(): Boolean {
        acquireLocks()
        try {
            val settings = EarSettings(
                dataDir = filesDir.path,
                name = Prefs.name(this),
                outputRate = AudioOut.nativeRate().toUInt(),
                wakeThreshold = Prefs.threshold(this),
            )
            val e = EarEngine(settings, host)
            engine = e
            current = e // avant start() : son premier statut doit passer
            e.start()
            return true
        } catch (ex: EarException.NotPaired) {
            Log.i(TAG, "pas d'appairage")
            EarNotification.postSetupAlert(this, getString(R.string.notif_setup_needed))
        } catch (ex: EarException.NoWakeword) {
            Log.i(TAG, "pas de mot d'éveil")
            EarNotification.postSetupAlert(this, getString(R.string.notif_setup_needed))
        } catch (ex: EarException) {
            Log.w(TAG, "démarrage du moteur : ${ex.message}")
            EarNotification.postSetupAlert(this, getString(R.string.notif_error, ex.message ?: ""))
        } catch (ex: Exception) {
            Log.w(TAG, "démarrage du moteur", ex)
            EarNotification.postSetupAlert(this, getString(R.string.notif_error, ex.message ?: ""))
        }
        engine = null
        current = null
        releaseLocks()
        return false
    }

    private fun stopEngine() {
        val e = engine
        current = null
        lastView = null
        try {
            e?.stop()
        } catch (ex: Exception) {
            Log.w(TAG, "arrêt du moteur", ex)
        }
        engine = null
        audioIn.stop()
        audioOut.stop()
    }

    private fun notify(view: EarView) {
        if (current == null) return
        getSystemService(android.app.NotificationManager::class.java)
            .notify(EarNotification.ID_LISTEN, EarNotification.build(this, view))
    }

    /** Verrous tenus pendant que le moteur tourne : traitement et Wi-Fi écran éteint, multicast. */
    private fun acquireLocks() {
        if (wakeLock == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "jarvis:ecoute")
                .apply { setReferenceCounted(false) }
        }
        val wifi = applicationContext.getSystemService(WifiManager::class.java)
        if (wifiLock == null) {
            @Suppress("DEPRECATION")
            wifiLock = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "jarvis")
                .apply { setReferenceCounted(false) }
        }
        if (multicastLock == null) {
            multicastLock = wifi.createMulticastLock("jarvis-mdns")
                .apply { setReferenceCounted(false) }
        }
        if (wakeLock?.isHeld != true) wakeLock?.acquire()
        if (wifiLock?.isHeld != true) wifiLock?.acquire()
        if (multicastLock?.isHeld != true) multicastLock?.acquire()
    }

    private fun releaseLocks() {
        for (lock in listOf<Any?>(wakeLock, wifiLock, multicastLock)) {
            try {
                when (lock) {
                    is PowerManager.WakeLock -> if (lock.isHeld) lock.release()
                    is WifiManager.WifiLock -> if (lock.isHeld) lock.release()
                    is WifiManager.MulticastLock -> if (lock.isHeld) lock.release()
                }
            } catch (ex: RuntimeException) {
                Log.w(TAG, "verrou", ex)
            }
        }
    }

    companion object {
        const val ACTION_START = "fr.mawena.jarvis.action.START"
        const val ACTION_STOP = "fr.mawena.jarvis.action.STOP"
        const val ACTION_MUTE = "fr.mawena.jarvis.action.MUTE"
        const val ACTION_UNMUTE = "fr.mawena.jarvis.action.UNMUTE"
        const val ACTION_HARD_MUTE = "fr.mawena.jarvis.action.HARD_MUTE"
        const val ACTION_REOPEN = "fr.mawena.jarvis.action.REOPEN"
        const val ACTION_RESTART = "fr.mawena.jarvis.action.RESTART"

        private const val TAG = "JarvisService"

        @Volatile private var alive = false
        @Volatile private var current: EarEngine? = null
        @Volatile private var lastView: EarView? = null

        /** Démarre l'écoute (à appeler pendant qu'une activité est visible). */
        fun start(ctx: Context) {
            ctx.applicationContext.startForegroundService(intent(ctx, ACTION_START))
        }

        /** Arrête l'écoute et retire la notification. */
        fun stop(ctx: Context) {
            // Service absent : rien à arrêter (et pas de premier plan juste pour s'arrêter).
            if (alive) ctx.applicationContext.startService(intent(ctx, ACTION_STOP))
        }

        /** Envoie une action (`ACTION_*`) au service. */
        fun send(ctx: Context, action: String) {
            val app = ctx.applicationContext
            when (action) {
                // Peuvent démarrer le service : doivent passer par le premier plan.
                ACTION_START, ACTION_RESTART -> app.startForegroundService(intent(ctx, action))
                ACTION_STOP -> stop(ctx)
                else -> if (alive) app.startService(intent(ctx, action))
            }
        }

        /** Vrai si le moteur tourne. */
        fun isRunning(): Boolean = current != null

        /** Dernier état reçu du moteur, `null` s'il ne tourne pas. */
        fun currentView(): EarView? = if (current != null) lastView else null

        private fun intent(ctx: Context, action: String) =
            Intent(ctx.applicationContext, ListenService::class.java).setAction(action)
    }
}
