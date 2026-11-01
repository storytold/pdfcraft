package ai.storyteller.pdfcraft.ui

import ai.storyteller.pdfcraft.PdfState
import ai.storyteller.pdfcraft.PdfViewModel
import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.weight
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.IconButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import java.nio.ByteBuffer
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable
fun PdfScreen(viewModel: PdfViewModel, onPickDocument: () -> Unit) {
    val state = viewModel.state
    Scaffold(topBar = { TopAppBar(title = { Text("PdfCraft") }) }) { padding ->
        Box(modifier = Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            when (state) {
                is PdfState.Empty -> Button(onClick = onPickDocument) { Text("Open a PDF") }
                is PdfState.Loading -> CircularProgressIndicator()
                is PdfState.Error ->
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(state.message)
                        Spacer(Modifier.height(16.dp))
                        Button(onClick = onPickDocument) { Text("Try another file") }
                    }
                is PdfState.Loaded ->
                    PageView(
                        state = state,
                        onPrev = { viewModel.goToPage(state.page - 1) },
                        onNext = { viewModel.goToPage(state.page + 1) },
                        onGoToPage = { viewModel.goToPage(it) },
                    )
            }
        }
    }
}

/** Renders the current page off the main thread and shows it, with pinch-to-zoom and a jump bar. */
@Composable
private fun PageView(state: PdfState.Loaded, onPrev: () -> Unit, onNext: () -> Unit, onGoToPage: (Int) -> Unit) {
    var bitmap by remember(state.document, state.page) { mutableStateOf<Bitmap?>(null) }
    var zoom by remember(state.page) { mutableFloatStateOf(1f) }
    var pan by remember(state.page) { mutableStateOf(Offset.Zero) }

    LaunchedEffect(state.document, state.page) {
        val rendered = withContext(Dispatchers.Default) { state.document.renderPage(state.page.toUInt(), 2.0f) }
        val decoded = Bitmap.createBitmap(rendered.width.toInt(), rendered.height.toInt(), Bitmap.Config.ARGB_8888)
        decoded.copyPixelsFromBuffer(ByteBuffer.wrap(rendered.rgba))
        bitmap = decoded
    }

    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxSize()) {
        Box(
            modifier =
                Modifier.weight(1f).fillMaxWidth().pointerInput(state.page) {
                    detectTransformGestures { _, panDelta, zoomDelta, _ ->
                        zoom = (zoom * zoomDelta).coerceIn(1f, 5f)
                        pan += panDelta
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            bitmap?.let {
                Image(
                    bitmap = it.asImageBitmap(),
                    contentDescription = "Page ${state.page + 1}",
                    modifier =
                        Modifier.graphicsLayer(
                            scaleX = zoom,
                            scaleY = zoom,
                            translationX = pan.x,
                            translationY = pan.y,
                        ),
                )
            } ?: CircularProgressIndicator()
        }
        Row(
            modifier = Modifier.padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onPrev, enabled = state.page > 0) { Text("<") }
            Text("${state.page + 1} / ${state.pageCount}")
            IconButton(onClick = onNext, enabled = state.page < state.pageCount - 1) { Text(">") }
        }
        if (state.pageCount > 1) {
            Slider(
                value = state.page.toFloat(),
                onValueChange = { onGoToPage(it.roundToInt()) },
                valueRange = 0f..(state.pageCount - 1).toFloat(),
                steps = (state.pageCount - 2).coerceAtLeast(0),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            )
        }
    }
}
