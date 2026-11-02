package io.github.dikshant363.pdfcraft

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.IBinder

/**
 * Keeps Read aloud going when PdfCraft leaves the screen, as the reference's reading service does:
 * a foreground media service with a notification (and lock-screen and headset controls) to play,
 * pause, skip 15 seconds and stop. The voice itself stays in the app's `ReadAloud`; this service
 * only holds the process in the foreground and passes the controls back through [controls].
 */
class ReadAloudService : Service() {
    /** What the notification's buttons do: set by the app while reading is on. */
    interface Controls {
        fun play()
        fun pause()
        fun skip(forward: Boolean)
        fun stop()
    }

    private var session: MediaSession? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm?.createNotificationChannel(NotificationChannel(CHANNEL, getString(R.string.common_tool_read_aloud), NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false) })
        session = MediaSession(this, "PdfCraft read aloud").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() { controls?.play() }
                override fun onPause() { controls?.pause() }
                override fun onSkipToNext() { controls?.skip(true) }
                override fun onSkipToPrevious() { controls?.skip(false) }
                override fun onFastForward() { controls?.skip(true) }
                override fun onRewind() { controls?.skip(false) }
                override fun onStop() { controls?.stop() }
            })
            isActive = true
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_PLAY -> controls?.play()
            ACTION_PAUSE -> controls?.pause()
            ACTION_BACK -> controls?.skip(false)
            ACTION_FORWARD -> controls?.skip(true)
            ACTION_STOP -> {
                controls?.stop()
                stopSelf()
                return START_NOT_STICKY
            }
        }
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: lastTitle
        val playing = intent?.getBooleanExtra(EXTRA_PLAYING, lastPlaying) ?: lastPlaying
        lastTitle = title
        lastPlaying = playing
        session?.setPlaybackState(
            PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE or PlaybackState.ACTION_REWIND or PlaybackState.ACTION_FAST_FORWARD or PlaybackState.ACTION_STOP)
                .setState(if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED, PlaybackState.PLAYBACK_POSITION_UNKNOWN, 1f)
                .build(),
        )
        val notification = notification(title, playing)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        } else {
            startForeground(NOTIFICATION, notification)
        }
        return START_NOT_STICKY
    }

    private fun notification(title: String, playing: Boolean): Notification {
        fun action(icon: Int, label: String, action: String) = Notification.Action.Builder(
            Icon.createWithResource(this, icon), label,
            PendingIntent.getService(this, action.hashCode(), Intent(this, ReadAloudService::class.java).setAction(action), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
        ).build()
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_read_aloud)
            .setContentTitle(title)
            .setContentText(getString(if (playing) R.string.service_notification_reading else R.string.service_notification_paused))
            .setContentIntent(open)
            .setOngoing(playing)
            .setDeleteIntent(PendingIntent.getService(this, 1, Intent(this, ReadAloudService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE))
            .addAction(action(android.R.drawable.ic_media_rew, getString(R.string.service_action_back), ACTION_BACK))
            .addAction(if (playing) action(android.R.drawable.ic_media_pause, getString(R.string.common_pause), ACTION_PAUSE) else action(android.R.drawable.ic_media_play, getString(R.string.common_play), ACTION_PLAY))
            .addAction(action(android.R.drawable.ic_media_ff, getString(R.string.service_action_forward), ACTION_FORWARD))
            .addAction(action(android.R.drawable.ic_menu_close_clear_cancel, getString(R.string.service_action_stop), ACTION_STOP))
            .setStyle(Notification.MediaStyle().setMediaSession(session?.sessionToken).setShowActionsInCompactView(0, 1, 2))
            .build()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // swiped away from recents: reading stops with the app
        controls?.stop()
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        session?.release()
        session = null
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "readAloud"
        private const val NOTIFICATION = 7
        private const val ACTION_PLAY = "io.github.dikshant363.pdfcraft.readaloud.PLAY"
        private const val ACTION_PAUSE = "io.github.dikshant363.pdfcraft.readaloud.PAUSE"
        private const val ACTION_BACK = "io.github.dikshant363.pdfcraft.readaloud.BACK"
        private const val ACTION_FORWARD = "io.github.dikshant363.pdfcraft.readaloud.FORWARD"
        private const val ACTION_STOP = "io.github.dikshant363.pdfcraft.readaloud.STOP"
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_PLAYING = "playing"
        private var lastTitle = ""
        private var lastPlaying = false

        /** The app's reading, while there is one. */
        @Volatile
        var controls: Controls? = null

        /** Show or update the notification (starting the service the first time). */
        fun show(context: Context, title: String, playing: Boolean) {
            val i = Intent(context, ReadAloudService::class.java).putExtra(EXTRA_TITLE, title).putExtra(EXTRA_PLAYING, playing)
            runCatching { context.startForegroundService(i) }
        }

        fun hide(context: Context) {
            context.stopService(Intent(context, ReadAloudService::class.java))
        }
    }
}
