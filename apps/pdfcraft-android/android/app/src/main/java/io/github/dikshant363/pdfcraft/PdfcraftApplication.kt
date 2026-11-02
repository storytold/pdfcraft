package io.github.dikshant363.pdfcraft

import android.app.Application
import android.content.ComponentCallbacks2
import io.github.dikshant363.pdfcraft.engine.Engine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async

class PdfcraftApplication : Application() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** The engine, loaded once per process on a background thread and shared by the activities. */
    val engine: Deferred<Result<Engine>> by lazy { scope.async { runCatching { Engine.open() } } }

    override fun onCreate() {
        super.onCreate()
        initText(this)
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (level >= ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN && engine.isCompleted) {
            engine.getCompleted().getOrNull()?.trimMemory()
        }
    }
}
