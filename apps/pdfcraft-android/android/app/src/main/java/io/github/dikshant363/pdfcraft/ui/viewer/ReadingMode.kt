package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import kotlinx.coroutines.flow.first
import io.github.dikshant363.pdfcraft.ui.HintToast
import androidx.compose.ui.Alignment
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.foundation.layout.Box
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

/**
 * Reading Mode, shown in the viewer in place of the pages, as the reference does: each page's
 * text reflowed to the screen width, page after page down the gutter. The viewer's bars stay
 * (a tap hides them). Pinch or double tap changes the text size.
 */
@Composable
fun ReadingModeContent(vm: AppViewModel, doc: OpenDocument, top: Dp, bottom: Dp, onTap: () -> Unit) {
    val c = pdfColors
    val night = doc.controller.nightMode
    val paper = if (night) Color(0xFF1E1E1E) else Color.White
    val ink = if (night) Color(0xFFE6E6E6) else Color(0xFF1B1B1B)
    // a file reopened in Reading Mode has its page pending (the page canvas never lays out here)
    val list = rememberLazyListState(initialFirstVisibleItemIndex = (vm.pendingPage ?: doc.viewer.currentPage).coerceIn(0, (doc.pageCount - 1).coerceAtLeast(0)))
    var size by remember { mutableFloatStateOf(17f) }
    var hint by remember { mutableStateOf<String?>(null) }
    val pinchHint = stringResource(R.string.viewer_reading_hint)
    LaunchedEffect(doc) {
        delay(400)
        hint = pinchHint
        delay(3000)
        hint = null
    }
    // each page's paragraphs, loaded as it comes into view
    val loaded = remember(doc, doc.info.generation) { mutableStateMapOf<Int, List<Paragraph>>() }
    val start = remember(doc) { list.firstVisibleItemIndex }
    var settled by remember(doc) { mutableStateOf(false) }
    LaunchedEffect(doc) {
        // the pages above the first one grow as their text arrives: once they have, come back to
        // the top of the page the viewer was on
        snapshotFlow { (maxOf(0, start - 1)..minOf(doc.pageCount - 1, start + 1)).all { loaded.containsKey(it) } }.first { it }
        androidx.compose.runtime.withFrameNanos { }
        list.scrollToItem(start)
        settled = true
    }
    // the page being read is kept pending for the canvas (and for reopening) and shown on it now if it can be
    LaunchedEffect(list, settled) {
        if (settled) snapshotFlow { list.firstVisibleItemIndex }.collect {
            vm.pendingPage = it
            doc.viewer.goToPage(it)
        }
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = list,
            modifier = Modifier.fillMaxSize().background(c.desk)
                .pointerInput(Unit) {
                    // only a pinch changes the size: one finger is left to scroll
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false)
                        while (true) {
                            val event = awaitPointerEvent()
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.isEmpty()) break
                            if (pressed.size > 1) {
                                val zoom = event.calculateZoom()
                                if (zoom != 1f) size = (size * zoom).coerceIn(10f, 40f)
                                event.changes.forEach { it.consume() }
                            }
                        }
                    }
                }
                .pointerInput(Unit) {
                    detectTapGestures(
                        onTap = { onTap() },
                        onDoubleTap = { size = if (size * 4f / 3f > 40f) 13f else size * 4f / 3f },
                    )
                },
            contentPadding = PaddingValues(top = top, bottom = bottom),
            verticalArrangement = Arrangement.spacedBy(if (c.dark) 3.dp else 4.dp),
        ) {
            items(doc.pageCount) { page ->
                LaunchedEffect(page, doc.info.generation) {
                    if (loaded.containsKey(page)) return@LaunchedEffect
                    vm.loadParagraphs(doc, page)
                    // the editable paragraphs when the engine finds them, else the text layer's
                    // lines joined into paragraphs (what most PDFs from browsers need)
                    val found = vm.paragraphs[page].orEmpty().ifEmpty { vm.reflowParagraphs(doc, page) }
                    // a page scrolled away mid-load was cancelled (the engine calls swallow that):
                    // keep nothing, so it loads again when it comes back
                    if (!isActive) return@LaunchedEffect
                    loaded[page] = found
                }
                val ps = loaded[page]
                // a page still loading holds a screen's height, so the list can't settle on the wrong page
                Column(Modifier.fillMaxWidth().then(if (ps == null) Modifier.fillParentMaxHeight() else Modifier.heightIn(min = 120.dp)).background(paper).padding(horizontal = 20.dp, vertical = 16.dp)) {
                    when {
                        ps == null -> Unit
                        ps.isEmpty() -> Text(stringResource(R.string.viewer_reading_no_text), color = ink.copy(alpha = 0.6f), fontSize = size.sp)
                        else -> {
                            val body = ps.map { it.size }.sorted().let { it[it.size / 2] }.coerceAtLeast(1f)
                            for ((i, p) in ps.withIndex()) {
                                // headings keep their weight over the body text
                                val heading = p.size > body * 1.25f
                                if (i > 0) Spacer(Modifier.height((size * 0.8f).dp))
                                Text(
                                    p.text.replace('\n', ' '),
                                    color = ink,
                                    fontSize = (size * if (heading) (p.size / body).coerceAtMost(1.8f) else 1f).sp,
                                    lineHeight = (size * 1.5f * if (heading) 1.3f else 1f).sp,
                                    fontWeight = if (heading || p.bold) FontWeight.SemiBold else FontWeight.Normal,
                                )
                            }
                        }
                    }
                }
            }
        }
        HintToast(hint, Modifier.padding(start = 16.dp, end = 16.dp, bottom = bottom + 48.dp), alignment = Alignment.BottomCenter, corner = 10.dp)
    }
}
