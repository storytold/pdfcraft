package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.draw.drawBehind
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.Spacer
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.res.stringResource
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.WidgetInfo
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.SheetRow
import io.github.dikshant363.pdfcraft.ui.SheetTitle
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlin.math.roundToInt

/** A field being filled in: the widget tapped and its current draft value. */
class FieldEdit(val doc: OpenDocument, val widget: WidgetInfo) {
    var currentText: String = widget.value.orEmpty()
    var isCommitted: Boolean = false

    fun commit(vm: AppViewModel): kotlinx.coroutines.Job? {
        if (!isCommitted && currentText != widget.value.orEmpty()) {
            isCommitted = true
            if (!widget.password) vm.rememberSuggestion(currentText)
            return vm.fillField(doc, widget.field, currentText)
        }
        return null
    }
}

/**
 * Filling in a field where it is: a text field over its box with the field toolbar (Reset,
 * Previous, Next) above the keyboard; a choice list for combo boxes and lists.
 */
@Composable
fun BoxScope.FieldEditor(vm: AppViewModel, edit: FieldEdit) {
    val c = pdfColors
    val w = edit.widget
    val doc = edit.doc
    if (w.kind == "combo" || w.kind == "list") {
        // as the reference: a dialog of radio choices that closes on a pick
        val options = w.options.chunked(2).filter { it.size == 2 }
        io.github.dikshant363.pdfcraft.ui.PdfDialog(onDismiss = { vm.fieldEdit = null }, title = null, confirm = null, dismiss = null, list = true) {
            LazyColumn(Modifier.heightIn(max = 420.dp)) {
                items(options) { (value, label) ->
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            vm.fieldEdit = null
                            vm.fillField(doc, w.field, value)
                        }.height(48.dp).padding(start = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        androidx.compose.material3.RadioButton(
                            selected = w.value == value, onClick = { vm.fieldEdit = null; vm.fillField(doc, w.field, value) },
                            colors = androidx.compose.material3.RadioButtonDefaults.colors(selectedColor = Color(0xFF1473E6), unselectedColor = c.icon),
                        )
                        Spacer(Modifier.width(12.dp))
                        Text(label.ifBlank { value }, color = c.text, fontSize = 17.sp)
                    }
                }
            }
        }
        return
    }
    var text by remember(w) { mutableStateOf(edit.currentText) }
    val focus = remember(w) { FocusRequester() }
    LaunchedEffect(w) {
        // bring the field into the upper part of the screen, clear of the keyboard
        val top = doc.viewer.pageToScreen(w.page.toInt(), w.rect[0], w.rect[1]).y
        val want = doc.viewer.padTop + doc.viewer.viewH * 0.12f
        if (top > doc.viewer.viewH * 0.4f || top < doc.viewer.padTop) doc.viewer.scrollBy(0f, want - top)
        focus.requestFocus()
    }
    val density = LocalDensity.current
    val page = w.page.toInt()
    val a = doc.viewer.pageToScreen(page, w.rect[0], w.rect[1])
    val b = doc.viewer.pageToScreen(page, w.rect[2], w.rect[3])
    val heightPx = (b.y - a.y).coerceAtLeast(with(density) { 14.dp.toPx() })
    // the field's own text size on the page (most forms set about 60 % of a one-line box)
    val fontSize = with(density) { (if (w.multiline) minOf(heightPx * 0.25f, 12f * doc.viewer.pixelsPerPoint(page)) else heightPx * 0.6f).toSp() }
    fun commit() {
        edit.commit(vm)
    }
    // a tap off the field commits it
    Box(Modifier.fillMaxSize().pointerInput(w) { detectTapGestures { commit(); vm.fieldEdit = null } })
    Box(Modifier.offset { IntOffset(a.x.roundToInt(), a.y.roundToInt()) }) {
        BasicTextField(
            value = text,
            onValueChange = { v ->
                val trimmed = w.maxLength?.let { v.take(it.toInt()) } ?: v
                text = trimmed
                edit.currentText = trimmed
            },
            singleLine = !w.multiline,
            textStyle = TextStyle(color = Color.Black, fontSize = fontSize, lineHeight = fontSize * 1.15f),
            cursorBrush = SolidColor(Color(0xFF1473E6)),
            visualTransformation = if (w.password) PasswordVisualTransformation() else VisualTransformation.None,
            keyboardOptions = KeyboardOptions(imeAction = if (w.multiline) ImeAction.Default else ImeAction.Next),
            keyboardActions = KeyboardActions(onNext = { commit(); vm.nextField(doc, w, forward = true) }),
            modifier = Modifier
                .size(with(density) { (b.x - a.x).coerceAtLeast(48f).toDp() }, with(density) { heightPx.toDp() })
                // the field being filled turns white inside a thin dark outline, as the reference's does
                .background(Color.White)
                .border(1.dp, Color(0xFF6E6E6E))
                .focusRequester(focus),
            decorationBox = { inner ->
                Box(Modifier.fillMaxSize().padding(horizontal = 4.dp), contentAlignment = if (w.multiline) Alignment.TopStart else Alignment.CenterStart) { inner() }
            },
        )
    }
    // words used in forms before, above the keyboard
    val suggestions = if (w.password) emptyList() else vm.suggestionsFor(text)
    if (suggestions.isNotEmpty()) {
        Row(Modifier.align(Alignment.BottomCenter).fillMaxWidth().imePadding().background(c.surface).height(44.dp), verticalAlignment = Alignment.CenterVertically) {
            for (sug in suggestions) {
                Text(sug, color = c.text, maxLines = 1, modifier = Modifier.weight(1f).clickable { text = sug; edit.currentText = sug }.padding(horizontal = 12.dp, vertical = 10.dp))
            }
        }
    }
    // the top bar's Reset and arrows act on this field
    DisposableEffect(edit) {
        val fillable = doc.widgets.filter { !it.readOnly && (it.kind == "text" || it.kind == "combo" || it.kind == "list") }
        val i = fillable.indexOfFirst { it.field == w.field && it.page == w.page && it.rect == w.rect }
        vm.fieldActions = FieldActions(
            reset = { text = ""; edit.currentText = "" },
            step = { forward -> commit(); vm.nextField(doc, w, forward) },
            canBack = i > 0,
            canForward = i in 0 until fillable.size - 1,
        )
        onDispose {
            commit()
            vm.fieldActions = null
        }
    }
}

/** What the field top bar's buttons do for the field being filled. */
class FieldActions(val reset: () -> Unit, val step: (Boolean) -> Unit, val canBack: Boolean = true, val canForward: Boolean = true)

/** The top bar while a field is filled, as the reference's: Reset at the left, previous and next at the right. */
@Composable
fun FieldTopBar(actions: FieldActions) {
    val c = pdfColors
    Row(
        Modifier.fillMaxWidth().height(TopBarHeight).background(c.surface).drawBehind {
            drawRect(c.divider, androidx.compose.ui.geometry.Offset(0f, size.height - 1.dp.toPx()), androidx.compose.ui.geometry.Size(size.width, 1.dp.toPx()))
        },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(stringResource(R.string.forms_reset), color = c.label, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold,
            modifier = Modifier.clickable { actions.reset() }.padding(horizontal = 28.dp, vertical = 14.dp))
        Spacer(Modifier.weight(1f))
        BarIcon(LucideIcons.ChevronLeft, stringResource(R.string.forms_previous_field), { actions.step(false) }, enabled = actions.canBack)
        Spacer(Modifier.width(4.dp))
        BarIcon(LucideIcons.ChevronRight, stringResource(R.string.forms_next_field), { actions.step(true) }, enabled = actions.canForward)
        Spacer(Modifier.width(16.dp))
    }
}

/** Every fillable field tinted light blue on the page, as the reference marks them. */
fun androidx.compose.ui.graphics.drawscope.DrawScope.drawFieldTints(doc: OpenDocument) {
    // a signed signature field is done: no tint over the signature
    val signed = doc.signatures.filter { it.signed }.mapTo(HashSet()) { it.field }
    for (w in doc.widgets) {
        if (w.readOnly || w.rect.size < 4 || w.kind == "button" || (w.kind == "signature" && w.field in signed)) continue
        val page = w.page.toInt()
        val a = doc.viewer.pageToScreen(page, w.rect[0], w.rect[1])
        val b = doc.viewer.pageToScreen(page, w.rect[2], w.rect[3])
        if (b.y < 0f || a.y > size.height) continue
        drawRect(Color(0x4D3D6BEB), topLeft = a, size = androidx.compose.ui.geometry.Size(b.x - a.x, b.y - a.y), blendMode = androidx.compose.ui.graphics.BlendMode.Multiply)
    }
}
