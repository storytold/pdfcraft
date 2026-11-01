package ai.storyteller.pdfcraft.ui

import ai.storyteller.pdfcraft.PdfState
import ai.storyteller.pdfcraft.PdfViewModel
import ai.storyteller.pdfcraft.RENDER_SCALE
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberModalBottomSheetState
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
import uniffi.pdfcraft_mobile_ffi.OutlineEntry

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfScreen(viewModel: PdfViewModel, onPickDocument: () -> Unit) {
    val state = viewModel.state
    var searchBarOpen by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var outlineOpen by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            Column {
                TopAppBar(
                    title = { Text((state as? PdfState.Loaded)?.title ?: "PdfCraft") },
                    actions = {
                        if (state is PdfState.Loaded) {
                            IconButton(onClick = { searchBarOpen = !searchBarOpen }) { Text("Find") }
                            if (state.outline.isNotEmpty()) {
                                IconButton(onClick = { outlineOpen = true }) { Text("TOC") }
                            }
                        }
                    },
                )
                if (searchBarOpen && state is PdfState.Loaded) {
                    FindBar(
                        query = query,
                        onQueryChange = {
                            query = it
                            viewModel.search(it)
                        },
                        matchCount = state.matches.size,
                        matchIndex = state.matchIndex,
                        onPrevious = viewModel::previousMatch,
                        onNext = viewModel::nextMatch,
                    )
                }
            }
        },
    ) { padding ->
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

    if (outlineOpen && state is PdfState.Loaded) {
        OutlineSheet(
            entries = state.outline,
            onSelect = {
                viewModel.goToPage(it)
                outlineOpen = false
            },
            onDismiss = { outlineOpen = false },
        )
    }
}

@Composable
private fun FindBar(
    query: String,
    onQueryChange: (String) -> Unit,
    matchCount: Int,
    matchIndex: Int,
    onPrevious: () -> Unit,
    onNext: () -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        TextField(value = query, onValueChange = onQueryChange, modifier = Modifier.weight(1f), singleLine = true)
        if (query.isNotBlank()) {
            Text(if (matchCount > 0) "${matchIndex + 1}/$matchCount" else "0/0")
            IconButton(onClick = onPrevious, enabled = matchCount > 0) { Text("<") }
            IconButton(onClick = onNext, enabled = matchCount > 0) { Text(">") }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun OutlineSheet(entries: List<OutlineEntry>, onSelect: (Int) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        entries.forEach { entry ->
            ListItem(
                headlineContent = { Text(entry.title) },
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .clickable { onSelect(entry.page.toInt()) }
                        .padding(start = (entry.depth * 16u).toInt().dp),
            )
        }
    }
}

/** Renders the current page off the main thread and shows it, with pinch-to-zoom, a jump bar and
 * the active search match (if any) baked into the bitmap so it pans/zooms with the page. */
@Composable
private fun PageView(state: PdfState.Loaded, onPrev: () -> Unit, onNext: () -> Unit, onGoToPage: (Int) -> Unit) {
    var bitmap by remember(state.document, state.page, state.activeMatch) { mutableStateOf<Bitmap?>(null) }
    var zoom by remember(state.page) { mutableFloatStateOf(1f) }
    var pan by remember(state.page) { mutableStateOf(Offset.Zero) }

    LaunchedEffect(state.document, state.page, state.activeMatch) {
        val rendered = withContext(Dispatchers.Default) { state.document.renderPage(state.page.toUInt(), RENDER_SCALE) }
        val decoded = Bitmap.createBitmap(rendered.width.toInt(), rendered.height.toInt(), Bitmap.Config.ARGB_8888)
        decoded.copyPixelsFromBuffer(ByteBuffer.wrap(rendered.rgba))
        val match = state.activeMatch
        if (match != null && match.page.toInt() == state.page) {
            val canvas = Canvas(decoded)
            val paint = Paint().apply { color = Color.argb(90, 255, 213, 0) }
            match.rects.forEach { canvas.drawRect(it.x0, it.y0, it.x1, it.y1, paint) }
        }
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
