package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.launch

/** A voice comment to record at [at]. */
class VoicePrompt(val doc: OpenDocument, val at: PagePoint)

/**
 * Record voice comment: recording starts at once, with a timer and a live waveform, and a stop
 * button. After stopping, the recording can be played back before it is posted, recorded again or
 * thrown away. Closing the sheet while recording keeps what was recorded and posts it.
 */
@Composable
fun VoiceSheet(vm: AppViewModel, prompt: VoicePrompt) {
    val c = pdfColors
    val scope = rememberCoroutineScope()
    val file = remember(prompt) { vm.voiceFile() }
    val recorder = remember(prompt) { VoiceRecorder(file, scope) }
    val player = remember(prompt) { VoicePlayer(scope) }
    var recorded by remember(prompt) { mutableStateOf(false) }
    var warned by remember(prompt) { mutableStateOf(false) }

    fun begin() {
        recorded = false
        warned = false
        player.release()
        if (!recorder.start()) {
            vm.voicePrompt = null
            vm.tell(text(R.string.voice_mic_failed))
        }
    }

    fun stop() = scope.launch {
        recorder.stop()
        recorded = player.load(file)
        if (!recorded) vm.tell(text(R.string.voice_nothing_recorded))
    }

    fun close() {
        if (recorder.recording) {
            scope.launch {
                recorder.stop()
                vm.postVoice(prompt, file)
            }
        } else {
            player.release()
            file.delete()
            vm.voicePrompt = null
        }
    }

    LaunchedEffect(prompt) { begin() }
    // stopped by the time limit
    LaunchedEffect(recorder.recording) {
        if (!recorder.recording && recorder.elapsedMs >= VOICE_MAX_MS && !recorded) stop()
    }
    LaunchedEffect(recorder.elapsedMs >= VOICE_WARN_MS) {
        if (recorder.elapsedMs >= VOICE_WARN_MS && !warned) {
            warned = true
            vm.tell(text(R.string.voice_limit_warning))
        }
    }
    DisposableEffect(prompt) { onDispose { player.release() } }

    PdfSheet(onDismiss = ::close, dim = true) {
        Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(R.string.voice_record_comment), color = c.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            TextButton(
                onClick = {
                    player.release()
                    vm.postVoice(prompt, file)
                },
                enabled = recorded,
            ) { Text(stringResource(R.string.common_post), color = if (recorded) c.accent else c.textSecondary, fontWeight = FontWeight.SemiBold) }
        }
        Text(
            when {
                recorder.recording -> stringResource(R.string.voice_recording)
                player.playing -> stringResource(R.string.voice_playing)
                recorded -> stringResource(R.string.voice_listen_first)
                else -> ""
            },
            color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp),
        )
        Spacer(Modifier.height(12.dp))
        if (!recorded) {
            Waveform(recorder.levels, null, c.accent, c.divider, Modifier.padding(horizontal = 20.dp))
            Spacer(Modifier.height(8.dp))
            Text(clock(recorder.elapsedMs), color = c.text, fontSize = 15.sp, modifier = Modifier.align(Alignment.CenterHorizontally))
            Spacer(Modifier.height(12.dp))
            RoundButton(LucideIcons.Square, stringResource(R.string.voice_stop_recording), c.red, Modifier.align(Alignment.CenterHorizontally)) { if (recorder.recording) stop() }
        } else {
            val progress = if (player.durationMs > 0) player.positionMs / player.durationMs.toFloat() else 0f
            Waveform(recorder.levels, progress, c.accent, c.divider, Modifier.padding(horizontal = 20.dp).pointerInput(Unit) {
                detectTapGestures { p -> player.seekTo(p.x / size.width) }
            })
            Spacer(Modifier.height(8.dp))
            Text("${clock(player.positionMs.toLong())} / ${clock(player.durationMs.toLong())}", color = c.text, fontSize = 15.sp, modifier = Modifier.align(Alignment.CenterHorizontally))
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
                LabelledButton(LucideIcons.Trash2, stringResource(R.string.common_delete)) {
                    player.release()
                    file.delete()
                    vm.voicePrompt = null
                }
                RoundButton(if (player.playing) LucideIcons.Pause else LucideIcons.Play, stringResource(if (player.playing) R.string.common_pause else R.string.common_play), c.accent) { player.toggle() }
                LabelledButton(LucideIcons.RotateCcw, stringResource(R.string.voice_record_again)) { begin() }
            }
        }
        Spacer(Modifier.height(20.dp))
    }
}

/**
 * Bars for the loudness of each 50 ms. While recording ([progress] null) the newest bars that fit;
 * afterwards the whole recording squeezed to the width, the part played so far in [played].
 */
@Composable
private fun Waveform(levels: List<Float>, progress: Float?, played: Color, rest: Color, modifier: Modifier = Modifier) {
    Canvas(modifier.fillMaxWidth().height(56.dp)) {
        val step = 4.dp.toPx()
        val n = (size.width / step).toInt().coerceAtLeast(1)
        val bars: List<Float> = when {
            progress == null -> levels.takeLast(n)
            levels.size <= n -> levels.toList()
            else -> List(n) { i ->
                val from = i * levels.size / n
                val to = ((i + 1) * levels.size / n).coerceIn(from + 1, levels.size)
                levels.subList(from, to).maxOrNull() ?: 0f
            }
        }
        val mid = size.height / 2
        bars.forEachIndexed { i, v ->
            val h = (Math.sqrt(v.toDouble()).toFloat() * size.height).coerceIn(2.dp.toPx(), size.height)
            val x = i * step + step / 2
            val color = if (progress == null || i < bars.size * progress) played else rest
            drawLine(color, Offset(x, mid - h / 2), Offset(x, mid + h / 2), 2.dp.toPx(), StrokeCap.Round)
        }
    }
}

@Composable
private fun RoundButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, color: Color, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Box(
        modifier.size(56.dp).clip(CircleShape).background(color).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(icon, null, tint = Color.White, modifier = Modifier.size(24.dp)) }
}

@Composable
private fun LabelledButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, onClick: () -> Unit) {
    val c = pdfColors
    Column(
        Modifier.width(88.dp).clickable(role = Role.Button, onClick = onClick).padding(vertical = 6.dp).semantics { contentDescription = label },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(icon, null, tint = c.icon, modifier = Modifier.size(24.dp))
        Spacer(Modifier.height(4.dp))
        Text(label, color = c.text, fontSize = 12.sp)
    }
}

/** A voice comment's player in its panel: play/pause, a bar to seek, and the time. */
@Composable
fun VoiceCommentPlayer(vm: AppViewModel, doc: OpenDocument, comment: CommentInfo) {
    val c = pdfColors
    val scope = rememberCoroutineScope()
    val player = remember(comment.page, comment.index) { VoicePlayer(scope) }
    var ready by remember(comment.page, comment.index) { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(comment.page, comment.index, doc.info.generation) {
        val f = vm.voiceRecording(doc, comment)
        ready = f != null && player.load(f)
    }
    DisposableEffect(comment.page, comment.index) { onDispose { player.release() } }
    val playLabel = stringResource(if (player.playing) R.string.common_pause else R.string.voice_play_audio)
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        if (ready == false) {
            Text(stringResource(R.string.voice_cannot_play), color = c.textSecondary, fontSize = 14.sp)
            return@Row
        }
        Box(
            Modifier.size(40.dp).clip(CircleShape).background(c.accent).clickable(enabled = ready == true, role = Role.Button) { player.toggle() }
                .semantics { contentDescription = playLabel },
            contentAlignment = Alignment.Center,
        ) { Icon(if (player.playing) LucideIcons.Pause else LucideIcons.Play, null, tint = Color.White, modifier = Modifier.size(20.dp)) }
        Spacer(Modifier.width(12.dp))
        val progress = if (player.durationMs > 0) player.positionMs / player.durationMs.toFloat() else 0f
        Box(
            Modifier.weight(1f).height(24.dp).pointerInput(Unit) { detectTapGestures { p -> player.seekTo(p.x / size.width) } },
            contentAlignment = Alignment.CenterStart,
        ) {
            Box(Modifier.fillMaxWidth().height(4.dp).clip(CircleShape).background(c.divider))
            Box(Modifier.fillMaxWidth(progress).height(4.dp).clip(CircleShape).background(c.accent))
        }
        Spacer(Modifier.width(12.dp))
        Text("${clock(player.positionMs.toLong())} / ${clock(player.durationMs.toLong())}", color = c.textSecondary, fontSize = 13.sp)
    }
}

/** The microphone was refused before: why it is needed, and a way to the app's settings. */
@Composable
fun MicRationaleDialog(vm: AppViewModel) {
    io.github.dikshant363.pdfcraft.ui.PdfDialog(
        onDismiss = { vm.dismissMicRationale(false) }, title = stringResource(R.string.voice_mic_title),
        confirm = stringResource(R.string.voice_open_settings), onConfirm = { vm.dismissMicRationale(true) },
        dismiss = stringResource(R.string.voice_not_now), onDismissButton = { vm.dismissMicRationale(false) },
    ) { Text(stringResource(R.string.voice_mic_rationale), color = io.github.dikshant363.pdfcraft.ui.pdfColors.text, fontSize = 14.sp) }
}

/** A comment's type as the lists name it (a Sound comment is a voice comment here). */
fun commentKindLabel(c: CommentInfo): String = if (c.subtype == "Sound") text(R.string.voice_comment_kind) else c.kind
