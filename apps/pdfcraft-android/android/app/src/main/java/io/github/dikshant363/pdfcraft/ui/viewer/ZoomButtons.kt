package io.github.dikshant363.pdfcraft.ui.viewer

import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How long the zoom buttons stay after the mouse comes near them while the bars are hidden. */
private const val ZOOM_BUTTONS_MS = 2500L

/** The corner a mouse brings the zoom buttons up from: this far from the right and from the bottom. */
val ZOOM_CORNER_WIDTH = 100.dp
val ZOOM_CORNER_HEIGHT = 172.dp

/**
 * Whether the app runs on a desktop: Samsung DeX, ChromeOS or an Android PC. The zoom buttons
 * show only there, as the reference's do.
 */
fun isDesktop(context: Context, config: Configuration): Boolean {
    val pm = context.packageManager
    return samsungDesktop(config) || pm.hasSystemFeature(PackageManager.FEATURE_PC) ||
        pm.hasSystemFeature("org.chromium.arc.device_management")
}

// Samsung's documented test for DeX: its configuration field matches the "enabled" constant
private fun samsungDesktop(config: Configuration): Boolean = runCatching {
    val cls = config.javaClass
    cls.getField("SEM_DESKTOP_MODE_ENABLED").getInt(cls) == cls.getField("semDesktopModeEnabled").getInt(config)
}.getOrDefault(false)

/** The zoom buttons' brief appearances while the bars are hidden. */
@Stable
class ZoomButtonsState(private val scope: CoroutineScope) {
    var showing by mutableStateOf(false)
        private set
    private var hideJob: Job? = null

    /** Show the buttons for a moment (the mouse came near, or the wheel zoomed). */
    fun poke() {
        showing = true
        hideJob?.cancel()
        hideJob = scope.launch {
            delay(ZOOM_BUTTONS_MS)
            showing = false
        }
    }

    fun hide() {
        hideJob?.cancel()
        showing = false
    }
}

/**
 * Zoom in over zoom out, at the bottom right, as the reference has them on a desktop. Each zooms
 * a step about the top left, as the reference's do; a button is dimmed at the end of the range.
 */
@Composable
fun ZoomButtons(state: ViewerState, visible: Boolean, bottom: Dp, onHover: () -> Unit, modifier: Modifier = Modifier) {
    AnimatedVisibility(visible, modifier, enter = fadeIn(), exit = fadeOut()) {
        Column(
            Modifier
                .padding(end = 50.dp, bottom = bottom)
                // a pointer resting on the buttons keeps them up
                .pointerInput(Unit) {
                    awaitPointerEventScope {
                        while (true) {
                            if (awaitPointerEvent().type == PointerEventType.Move) onHover()
                        }
                    }
                },
            verticalArrangement = Arrangement.spacedBy(9.dp),
        ) {
            ZoomButton(LucideIcons.Plus, stringResource(R.string.viewer_zoom_in), state.canZoomIn) {
                state.zoomStep(1, androidx.compose.ui.geometry.Offset.Zero)
            }
            ZoomButton(LucideIcons.Minus, stringResource(R.string.viewer_zoom_out), state.canZoomOut) {
                state.zoomStep(-1, androidx.compose.ui.geometry.Offset.Zero)
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ZoomButton(icon: ImageVector, label: String, enabled: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = { PlainTooltip { Text(label) } },
        state = rememberTooltipState(),
    ) {
        Box(
            Modifier
                .size(40.dp)
                .shadow(4.dp, CircleShape)
                .clip(CircleShape)
                .background(c.surfaceRaised)
                .clickable(enabled = enabled, onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, label, tint = c.icon, modifier = Modifier.size(22.dp).alpha(if (enabled) 1f else 0.38f))
        }
    }
}
