package io.github.dikshant363.pdfcraft.ui.viewer

import android.content.Context
import android.content.Intent
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupProperties
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.ReadingBlock
import io.github.dikshant363.pdfcraft.core.TextRange
import io.github.dikshant363.pdfcraft.engine.Engine
import io.github.dikshant363.pdfcraft.engine.toBitmap
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.Locale
import kotlin.math.roundToInt

/**
 * Read Aloud: the document read out by the phone's own text-to-speech voice, paragraph by
 * paragraph from [startPage] on. The paragraph being read and its current word are marked on the
 * page, which scrolls to follow them; back and forward move about 15 seconds of reading.
 */
@Stable
class ReadAloud(
    context: Context,
    private val engine: Engine,
    private val doc: OpenDocument,
    private val scope: CoroutineScope,
    startPage: Int,
    private val onEnd: () -> Unit,
    /** Where this document's reading position is kept between sessions. */
    private val key: String = "",
    /** Start inside [startPage] at this glyph (the selection's first word), not its top. */
    private val startGlyph: UInt? = null,
    /** Pick up a stopped reading: paragraph and character within [startPage]. */
    private val resumeAt: Pair<Int, Int>? = null,
) {
    var page by mutableIntStateOf(startPage)
        private set

    /** The document's title, for the reading notification. */
    val title: String get() = doc.title
    var playing by mutableStateOf(false)
        private set
    var finished by mutableStateOf(false)
        private set
    var speed by mutableFloatStateOf(prefs(context).getFloat("speed", 1f))
        private set
    var problem by mutableStateOf<String?>(null)
        private set

    /** The language the voice reads in. */
    var language by mutableStateOf<Locale>(Locale.getDefault())
        private set

    /** The languages the phone's voice can read, by name. */
    fun languages(): List<Locale> = runCatching { tts.availableLanguages.orEmpty().sortedBy { it.getDisplayName(Locale.getDefault()) } }.getOrDefault(emptyList())

    /** Read in [locale] from now on (remembered), going on from where the voice is. */
    fun chooseLanguage(locale: Locale) {
        language = locale
        tts.language = locale
        prefs(prefsCtx).edit().putString("lang", locale.toLanguageTag()).apply()
        if (playing) speakCurrent()
    }

    /** The lines of the paragraph being read and of its current word (points, flattened x0, y0, x1, y1). */
    var blockRects by mutableStateOf<List<Float>>(emptyList())
        private set
    var wordRects by mutableStateOf<List<Float>>(emptyList())
        private set

    private val prefsCtx = context.applicationContext
    private var blocks: List<ReadingBlock> = emptyList()
    private var index = 0
    /** Where in the current paragraph the voice is (characters), and where its utterance began. */
    private var offset = 0
    private var base = 0
    private var ready = false

    private val tts: TextToSpeech = TextToSpeech(context.applicationContext) { status ->
        scope.launch {
            if (status != TextToSpeech.SUCCESS) {
                problem = text(R.string.read_aloud_no_voice)
                return@launch
            }
            ready = true
            // the language chosen before (Voice), else the phone's
            tts.language = prefs(prefsCtx).getString("lang", null)?.let { Locale.forLanguageTag(it) } ?: Locale.getDefault()
            language = tts.language ?: Locale.getDefault()
            tts.setSpeechRate(speed)
            loadPage(page)
            placeStart()
            play()
        }
    }

    /** The document's words (CJK characters count one each), once counted; 250 a page until then. */
    private var totalWords by mutableIntStateOf(0)

    init {
        // count the words of every page for the time estimate, as the reference does
        scope.launch {
            totalWords = withContext(Dispatchers.Default) {
                (0 until doc.pageCount).sumOf { p -> countWords(runCatching { engine.session.pagePlainText(doc.id, p.toUInt()) }.getOrDefault("")) }
            }
        }
        tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}

            @Deprecated("Deprecated in Java")
            override fun onError(utteranceId: String?) {}

            override fun onRangeStart(utteranceId: String?, start: Int, end: Int, frame: Int) {
                scope.launch {
                    if (utteranceId != utteranceFor()) return@launch
                    offset = base + start
                    markWord(base + start, base + end)
                }
            }

            override fun onDone(utteranceId: String?) {
                scope.launch {
                    if (!playing || utteranceId != utteranceFor()) return@launch
                    index++
                    offset = 0
                    if (index < blocks.size) speakCurrent() else nextPage()
                }
            }
        })
    }

    private fun utteranceFor() = "p$page-$index-$base"

    private suspend fun loadPage(p: Int) {
        page = p
        index = 0
        offset = 0
        blocks = withContext(Dispatchers.Default) { runCatching { engine.session.readingBlocks(doc.id, p.toUInt()) }.getOrDefault(emptyList()) }
        if (doc.viewer.currentPage != p) doc.viewer.goToPage(p)
    }

    /** Move to the selection's word, or to where a stopped reading was, within the loaded page. */
    private fun placeStart() {
        val g = startGlyph
        if (g != null) {
            val i = blocks.indexOfFirst { g >= it.range.start && g < it.range.end }
            if (i >= 0) {
                index = i
                offset = blocks[i].glyphOfChar.indexOfFirst { it >= g }.coerceAtLeast(0)
            }
            return
        }
        resumeAt?.let { (i, o) ->
            if (i in blocks.indices) {
                index = i
                offset = o.coerceIn(0, blocks[i].text.length)
            }
        }
    }

    private suspend fun nextPage() {
        var p = page + 1
        while (p < doc.pageCount) {
            loadPage(p)
            if (blocks.isNotEmpty()) {
                speakCurrent()
                return
            }
            p++
        }
        playing = false
        finished = true
        blockRects = emptyList()
        wordRects = emptyList()
    }

    private fun speakCurrent() {
        val block = blocks.getOrNull(index) ?: return
        base = offset.coerceIn(0, block.text.length)
        scope.launch {
            val rects = withContext(Dispatchers.Default) { runCatching { engine.session.textRects(doc.id, block.range) }.getOrDefault(emptyList()) }
            blockRects = rects
            wordRects = emptyList()
            follow(rects)
        }
        tts.speak(block.text.substring(base), TextToSpeech.QUEUE_FLUSH, null, utteranceFor())
    }

    /** The word the voice is on, from characters [start] to [end] of the paragraph. */
    private suspend fun markWord(start: Int, end: Int) {
        val block = blocks.getOrNull(index) ?: return
        val first = block.glyphOfChar.getOrNull(start) ?: return
        val last = block.glyphOfChar.getOrNull((end - 1).coerceAtLeast(start)) ?: first
        val range = TextRange(page.toUInt(), first, last + 1u)
        wordRects = withContext(Dispatchers.Default) { runCatching { engine.session.textRects(doc.id, range) }.getOrDefault(emptyList()) }
    }

    /** Scroll so a paragraph that starts out of view sits just under the top bar. */
    private fun follow(rects: List<Float>) {
        if (rects.size < 4) return
        val v = doc.viewer
        val top = v.pageToScreen(page, rects[0], rects[1]).y
        if (top < v.padTop || top > v.viewH - v.padBottom - 120f) v.goToPage(page, (rects[1] - 24f).coerceAtLeast(0f))
    }

    fun play() {
        if (!ready) return
        if (finished) {
            finished = false
            scope.launch {
                loadPage(0)
                playing = true
                if (blocks.isEmpty()) nextPage() else speakCurrent()
            }
            return
        }
        playing = true
        if (blocks.isEmpty()) scope.launch { nextPage() } else speakCurrent()
    }

    fun pause() {
        playing = false
        tts.stop()
    }

    /**
     * About 15 seconds of reading back or forward: the words a voice says in that time at the
     * current speed (some 32 at 1.0x), counted across paragraphs and pages.
     */
    fun skip(forward: Boolean) {
        scope.launch {
            tts.stop()
            var words = (15f * WORDS_PER_MINUTE / 60f * speed).roundToInt().coerceAtLeast(1)
            if (forward) {
                while (words > 0) {
                    val text = blocks.getOrNull(index)?.text
                    if (text == null) {
                        if (page + 1 >= doc.pageCount) break
                        loadPage(page + 1)
                        continue
                    }
                    val next = text.indexOf(' ', offset + 1)
                    if (next < 0) {
                        index++
                        offset = 0
                        if (index >= blocks.size) {
                            if (page + 1 >= doc.pageCount) {
                                pause()
                                finished = true
                                return@launch
                            }
                            loadPage(page + 1)
                        }
                    } else {
                        offset = next + 1
                    }
                    words--
                }
            } else {
                while (words > 0) {
                    val text = blocks.getOrNull(index)?.text.orEmpty()
                    val prev = if (offset > 1) text.lastIndexOf(' ', offset - 2) else -1
                    if (prev < 0 && offset > 0) {
                        offset = 0
                    } else if (prev < 0) {
                        if (index > 0) {
                            index--
                            offset = blocks.getOrNull(index)?.text?.length ?: 0
                        } else if (page > 0) {
                            loadPage(page - 1)
                            index = (blocks.size - 1).coerceAtLeast(0)
                            offset = blocks.getOrNull(index)?.text?.length ?: 0
                        } else {
                            break
                        }
                    } else {
                        offset = prev + 1
                    }
                    words--
                }
            }
            if (playing) speakCurrent()
        }
    }

    fun setRate(r: Float) {
        speed = r
        tts.setSpeechRate(r)
        prefs(prefsCtx).edit().putFloat("speed", r).apply()
        if (playing) speakCurrent()
    }

    fun stop() {
        playing = false
        // keep the place for next time (the reference offers to resume); a finished reading starts
        // over at the beginning next time, as the reference's does
        if (key.isNotEmpty()) {
            val (p, i, o) = if (finished) START_OVER else Triple(page, index, offset)
            prefs(prefsCtx).edit().putString("pos:$key", "$p,$i,$o").apply()
        }
        runCatching {
            tts.stop()
            tts.shutdown()
        }
        blockRects = emptyList()
        wordRects = emptyList()
        onEnd()
    }

    /** How far through the document reading is, 0..1. */
    val progress: Float
        get() {
            if (doc.pageCount == 0) return 0f
            val within = if (blocks.isEmpty()) 0f else (index + offset.toFloat() / (blocks.getOrNull(index)?.text?.length ?: 1).coerceAtLeast(1)) / blocks.size
            return ((page + within) / doc.pageCount).coerceIn(0f, 1f)
        }

    /** Reading time in all, estimated from the document's words (about 250 a page until they are counted). */
    val estimatedTotalMs: Long get() {
        val words = if (totalWords > 0) totalWords.toLong() else doc.pageCount * 250L
        return (words * 60_000L / (WORDS_PER_MINUTE * speed)).toLong()
    }

    companion object {
        val SPEEDS = listOf(0.5f, 0.75f, 1f, 1.25f, 1.5f, 1.75f, 2f)

        /** The saved position of a reading that went to the end: the top of the first page. */
        val START_OVER = Triple(0, 0, 0)
        /** A voice's pace at 1.0x, for turning 15 seconds into words. */
        private const val WORDS_PER_MINUTE = 128f
        private fun prefs(c: Context) = c.getSharedPreferences("readAloud", Context.MODE_PRIVATE)

        /** Words in [text]: runs of letters or digits, with each CJK character a word of its own. */
        fun countWords(text: String): Int {
            var n = 0
            var inWord = false
            for (ch in text) {
                val cjk = Character.UnicodeScript.of(ch.code) in setOf(Character.UnicodeScript.HAN, Character.UnicodeScript.HIRAGANA, Character.UnicodeScript.KATAKANA, Character.UnicodeScript.HANGUL)
                when {
                    cjk -> { n++; inWord = false }
                    ch.isLetterOrDigit() -> { if (!inWord) n++; inWord = true }
                    else -> inWord = false
                }
            }
            return n
        }

        /** Where reading of a document stopped: page, paragraph and character, if it did. */
        fun savedPosition(c: Context, key: String): Triple<Int, Int, Int>? {
            val v = prefs(c).getString("pos:$key", null)?.split(',')?.mapNotNull { it.toIntOrNull() } ?: return null
            return if (v.size == 3) Triple(v[0], v[1], v[2]) else null
        }

        fun forgetPosition(c: Context, key: String) = prefs(c).edit().remove("pos:$key").apply()
    }
}

/** "1.0x", "1.25x": the speed as the player shows it. */
internal fun speedLabel(s: Float): String = if (s * 10f % 1f == 0f) "%.1fx".format(Locale.US, s) else "%.2fx".format(Locale.US, s)

/** The paragraph being read, pale yellow, and its current word, stronger, laid over the page. */
fun DrawScope.drawReadAloud(doc: OpenDocument, r: ReadAloud) {
    fun mark(rects: List<Float>, color: Color, pad: Float) {
        var i = 0
        while (i + 3 < rects.size) {
            val a = doc.viewer.pageToScreen(r.page, rects[i], rects[i + 1])
            val b = doc.viewer.pageToScreen(r.page, rects[i + 2], rects[i + 3])
            drawRoundRect(color, Offset(a.x - pad, a.y - pad), Size(b.x - a.x + 2 * pad, b.y - a.y + 2 * pad), CornerRadius(2f * density), blendMode = BlendMode.Multiply)
            i += 4
        }
    }
    mark(r.blockRects, Color(0xFFFFFFBB), 1f * density)
    mark(r.wordRects, Color(0xFFF8D904), 1.5f * density)
}

/**
 * The Read Aloud player, as the reference lays it out: a thin progress track along its top, then
 * the round button that opens the full player, back 15 seconds, play or pause, forward 15
 * seconds, and the speed.
 */
@Composable
fun BoxScope.ReadAloudBar(vm: io.github.dikshant363.pdfcraft.ui.AppViewModel, doc: OpenDocument, r: ReadAloud) {
    val c = pdfColors
    var speeds by remember { mutableStateOf(false) }
    var expanded by remember { mutableStateOf(false) }
    val openPlayer = stringResource(R.string.read_aloud_open_player)
    Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().background(c.surface)) {
        ProgressTrack(r.progress, Modifier.fillMaxWidth().height(6.dp))
        Box(Modifier.fillMaxWidth().height(0.5.dp).background(c.divider))
        Row(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 16.dp, bottom = 24.dp).height(56.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.size(40.dp).clip(CircleShape).background(c.pageCard).clickable(role = Role.Button) { expanded = true }
                    .semantics { contentDescription = openPlayer },
                contentAlignment = Alignment.Center,
            ) { Icon(LucideIcons.ChevronUp, null, tint = c.label, modifier = Modifier.size(24.dp)) }
            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
                SkipButton(back = true, size = 43.dp) { r.skip(false) }
                PlayerButton(if (r.playing) LucideIcons.Pause else LucideIcons.Play, stringResource(if (r.playing) R.string.common_pause else if (r.finished) R.string.read_aloud_read_again else R.string.common_play), 43.dp) {
                    if (r.playing) r.pause() else r.play()
                }
                SkipButton(back = false, size = 43.dp) { r.skip(true) }
            }
            Box {
                Text(speedLabel(r.speed), color = c.label, fontSize = 15.sp, modifier = Modifier.clickable(role = Role.Button) { speeds = true }.padding(vertical = 10.dp, horizontal = 2.dp))
                if (speeds) SpeedPicker(r) { speeds = false }
            }
        }
        if (r.problem != null) Text(r.problem.orEmpty(), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(start = 20.dp, end = 20.dp, bottom = 8.dp))
        Box(Modifier.fillMaxWidth().windowInsetsBottomHeight(androidx.compose.foundation.layout.WindowInsets.navigationBars).background(c.navBand))
    }
    if (expanded) ReadAloudFull(vm, doc, r) { expanded = false }
}

/** The progress track: the part read in the accent blue, the rest grey, rounded ends. */
@Composable
private fun ProgressTrack(progress: Float, modifier: Modifier) {
    val c = pdfColors
    val rest = if (c.dark) Color(0xFF6A6A6A) else Color(0xFFB1B1B1)
    Canvas(modifier) {
        val r = CornerRadius(size.height / 2f)
        drawRoundRect(rest, cornerRadius = r)
        drawRoundRect(c.toolAccent, size = Size(size.width * progress.coerceIn(0f, 1f), size.height), cornerRadius = r)
    }
}

@Composable
private fun PlayerButton(icon: ImageVector, description: String, size: androidx.compose.ui.unit.Dp, iconSize: androidx.compose.ui.unit.Dp = 24.dp, onClick: () -> Unit) {
    Box(
        Modifier.size(size).clip(CircleShape).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { Icon(icon, null, tint = pdfColors.label, modifier = Modifier.size(iconSize)) }
}

/** Back or forward 15 seconds: a turning arrow with "15" in it. */
@Composable
private fun SkipButton(back: Boolean, size: androidx.compose.ui.unit.Dp, iconSize: androidx.compose.ui.unit.Dp = 26.dp, onClick: () -> Unit) {
    val c = pdfColors
    val label = stringResource(if (back) R.string.read_aloud_back_15 else R.string.read_aloud_forward_15)
    Box(
        Modifier.size(size).clip(CircleShape).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(if (back) LucideIcons.RotateCcw else LucideIcons.RotateCw, null, tint = c.label, modifier = Modifier.size(iconSize))
        Text("15", color = c.label, fontSize = (iconSize.value * 0.3f).sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.offset(y = 0.5.dp))
    }
}

/** The speeds, in a card at the lower right: tap one to use it. */
@Composable
private fun SpeedPicker(r: ReadAloud, onDismiss: () -> Unit) {
    val c = pdfColors
    Popup(alignment = Alignment.BottomEnd, offset = androidx.compose.ui.unit.IntOffset(0, 0), onDismissRequest = onDismiss, properties = PopupProperties(focusable = true)) {
        Box(Modifier.fillMaxSize().background(c.scrim).pointerInput(Unit) { detectTapGestures { onDismiss() } }) {
            Column(
                Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(end = 24.dp, bottom = 76.dp).width(200.dp)
                    .shadow(2.dp, RoundedCornerShape(8.dp)).background(c.surface, RoundedCornerShape(8.dp)).padding(vertical = 12.dp),
            ) {
                for (s in ReadAloud.SPEEDS) {
                    Row(
                        Modifier.fillMaxWidth().clickable { r.setRate(s); onDismiss() }.padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(speedLabel(s), color = c.label, fontSize = 16.sp, modifier = Modifier.weight(1f))
                        if (s == r.speed) Icon(LucideIcons.Check, null, tint = c.toolAccent, modifier = Modifier.size(20.dp))
                    }
                }
            }
        }
    }
}

/**
 * The full player: the page being read on a card, the document's name, the progress with the time
 * gone and left, large back, play and forward buttons, then Voice (the phone's text-to-speech
 * settings) and the speed.
 */
@Composable
private fun ReadAloudFull(vm: io.github.dikshant363.pdfcraft.ui.AppViewModel, doc: OpenDocument, r: ReadAloud, onClose: () -> Unit) {
    val c = pdfColors
    val context = LocalContext.current
    var thumb by remember(r.page) { mutableStateOf<ImageBitmap?>(null) }
    var speeds by remember { mutableStateOf(false) }
    var voices by remember { mutableStateOf(false) }
    val closePlayer = stringResource(R.string.read_aloud_close_player)
    LaunchedEffect(r.page) {
        val eng = vm.engine ?: return@LaunchedEffect
        val size = doc.info.pages.getOrNull(r.page) ?: return@LaunchedEffect
        val scale = 480f / maxOf(size.width, size.height).coerceAtLeast(1f)
        thumb = runCatching { withContext(eng.thumbnails) { eng.session.renderPage(doc.id, r.page.toUInt(), scale, null).toBitmap().asImageBitmap() } }.getOrNull()
    }
    // the language sheet sits over the player, not in its column (which the player fills)
    Box(Modifier.fillMaxSize()) {
    PdfSheet(onDismiss = onClose, maxHeightFraction = 1f, fill = true) {
        Column(Modifier.fillMaxSize().padding(horizontal = 20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.fillMaxWidth()) {
                Box(
                    Modifier.size(40.dp).clip(CircleShape).background(c.pageCard).clickable(role = Role.Button, onClick = onClose)
                        .semantics { contentDescription = closePlayer },
                    contentAlignment = Alignment.Center,
                ) { Icon(LucideIcons.ChevronDown, null, tint = c.label, modifier = Modifier.size(22.dp)) }
            }
            Spacer(Modifier.weight(1f))
            Box(Modifier.heightIn(max = 240.dp).widthIn(max = 240.dp).shadow(4.dp, RoundedCornerShape(8.dp)).background(Color.White, RoundedCornerShape(8.dp)).clip(RoundedCornerShape(8.dp))) {
                thumb?.let { Image(it, null, contentScale = ContentScale.Fit, modifier = Modifier.heightIn(max = 240.dp)) }
            }
            Spacer(Modifier.height(24.dp))
            Text(doc.title, color = c.label, fontSize = 22.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.height(24.dp))
            ProgressTrack(r.progress, Modifier.fillMaxWidth().height(6.dp))
            Row(Modifier.fillMaxWidth().padding(top = 6.dp)) {
                val gone = (r.estimatedTotalMs * r.progress).toLong()
                Text(readClock(gone), color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.weight(1f))
                Text("-" + readClock(r.estimatedTotalMs - gone), color = c.textSecondary, fontSize = 11.sp)
            }
            Spacer(Modifier.height(16.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                SkipButton(back = true, size = 58.dp, iconSize = 32.dp) { r.skip(false) }
                Box(Modifier.size(80.dp).clip(CircleShape).background(c.divider), contentAlignment = Alignment.Center) {
                    PlayerButton(if (r.playing) LucideIcons.Pause else LucideIcons.Play, stringResource(if (r.playing) R.string.common_pause else R.string.common_play), 80.dp, iconSize = 40.dp) {
                        if (r.playing) r.pause() else r.play()
                    }
                }
                SkipButton(back = false, size = 58.dp, iconSize = 32.dp) { r.skip(true) }
            }
            Spacer(Modifier.weight(1f))
            Row(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalArrangement = Arrangement.SpaceAround, verticalAlignment = Alignment.CenterVertically) {
                Column(
                    Modifier.clickable { voices = true }.padding(8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Icon(LucideIcons.AudioLines, null, tint = c.icon, modifier = Modifier.size(24.dp))
                    Text(stringResource(R.string.read_aloud_voice), color = c.textSecondary, fontSize = 13.sp)
                }
                Box {
                    Text(speedLabel(r.speed), color = c.textSecondary, fontSize = 16.sp, modifier = Modifier.clickable { speeds = true }.padding(8.dp))
                    if (speeds) SpeedPicker(r) { speeds = false }
                }
            }
        }
    }
    if (voices) LanguageSheet(r, onVoiceSettings = {
        voices = false
        runCatching { context.startActivity(Intent("com.android.settings.TTS_SETTINGS").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }) { voices = false }
    }
}

/** Choose the language to read in: the ones the phone's voice has, then its voice settings. */
@Composable
private fun LanguageSheet(r: ReadAloud, onVoiceSettings: () -> Unit, onDismiss: () -> Unit) {
    val c = pdfColors
    val langs = remember { r.languages() }
    // opened at the language in use (a couple of rows above it showing)
    val current = remember(langs, r.language) { languageIndex(langs, r.language) }
    val list = androidx.compose.foundation.lazy.rememberLazyListState(initialFirstVisibleItemIndex = (current - 2).coerceAtLeast(0))
    io.github.dikshant363.pdfcraft.ui.PdfSheet(onDismiss = onDismiss, dim = true) {
        io.github.dikshant363.pdfcraft.ui.SheetTitle(stringResource(R.string.read_aloud_choose_language))
        androidx.compose.foundation.lazy.LazyColumn(Modifier.heightIn(max = 420.dp), state = list) {
            items(langs.size) { i ->
                val l = langs[i]
                io.github.dikshant363.pdfcraft.ui.SheetRow(null, l.getDisplayName(Locale.getDefault()), { r.chooseLanguage(l); onDismiss() }, checked = i == current)
            }
        }
        io.github.dikshant363.pdfcraft.ui.SheetRow(LucideIcons.Settings, stringResource(R.string.read_aloud_voice_settings), onVoiceSettings)
        Spacer(Modifier.height(8.dp))
    }
}

/**
 * Where [current] is in [langs]: the same language and country, else the same language. Voices
 * often list their languages by three-letter codes ("eng", "USA"), so both are compared that way.
 * -1 when it isn't there.
 */
internal fun languageIndex(langs: List<Locale>, current: Locale): Int {
    fun Locale.lang() = runCatching { isO3Language }.getOrDefault(language).lowercase(Locale.ROOT)
    fun Locale.place() = runCatching { isO3Country }.getOrDefault(country).uppercase(Locale.ROOT)
    val exact = langs.indexOfFirst { it.lang() == current.lang() && it.place() == current.place() }
    return if (exact >= 0) exact else langs.indexOfFirst { it.lang() == current.lang() }
}

private fun readClock(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(Locale.US, s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(Locale.US, s / 60, s % 60)
}
