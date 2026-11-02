package io.github.dikshant363.pdfcraft.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutLinearInEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.ui.composed
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.ui.focus.focusRequester
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.border
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import kotlin.math.roundToInt
import kotlinx.coroutines.launch

/** Motion for sheets: in rising from below, out dropping away. */
object PdfMotion {
    const val SHEET_IN_MS = 250
    const val SHEET_OUT_MS = 200
    const val BARS_MS = 300
}

/** An icon button of the bars: a 46 dp target with a 24 dp icon, a pressed background when [selected]. */
@Composable
fun BarIcon(
    icon: ImageVector,
    description: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    tint: Color = pdfColors.icon,
    selected: Boolean = false,
    enabled: Boolean = true,
    size: Dp = 46.dp,
    iconSize: Dp = 24.dp,
) {
    Box(
        modifier
            .size(size)
            .clip(RoundedCornerShape(6.dp))
            .background(if (selected) pdfColors.pressed else Color.Transparent)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = if (enabled) tint else tint.copy(alpha = 0.38f), modifier = Modifier.size(iconSize))
    }
}

/** The thin vertical line between groups of a bar. */
@Composable
fun BarDivider(height: Dp = 26.dp) {
    Box(Modifier.width(1.dp).height(height).background(pdfColors.divider))
}

/** Whether the sheet being shown is coming in or going ([SheetHost] sets it). */
private val LocalSheetVisible = staticCompositionLocalOf<MutableTransitionState<Boolean>?> { null }

private class Last<T>(var value: T? = null)

/**
 * Shows [key]'s sheet ([content]; nothing while [key] is null), keeping the last one composed
 * while it animates out, so a sheet closed from anywhere leaves smoothly.
 */
@Composable
fun <T : Any> SheetHost(key: T?, content: @Composable (T) -> Unit) {
    val visible = remember { MutableTransitionState(false) }
    val last = remember { Last<T>() }
    if (key != null) last.value = key
    SideEffect { visible.targetState = key != null }
    val shown = last.value
    if (shown != null && (visible.currentState || visible.targetState)) {
        CompositionLocalProvider(LocalSheetVisible provides visible) { content(shown) }
    }
}

/**
 * A bottom sheet as the reference's viewer uses them: the page stays undimmed behind it (a tap
 * there closes it, as does Back), a drag handle at the top, and it can be pulled down to close.
 * [maxHeightFraction] caps its height as a fraction of the screen.
 */
@Composable
fun PdfSheet(
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    dim: Boolean = false,
    maxHeightFraction: Float = 0.9f,
    /** Room kept free above the sheet however tall it grows (the viewer's top bar). */
    topReserve: Dp = 0.dp,
    /** Whether the sheet paints the band behind the navigation buttons (not when it sits on a bar). */
    bottomBand: Boolean = true,
    /** The sheet's colour, when not the usual surface (the comment panel's). */
    background: Color? = null,
    /** The strip holding the drag handle (the comment panel's is shorter). */
    handleHeight: Dp = 24.dp,
    /** Whether the sheet is [maxHeightFraction] of the screen tall above the navigation buttons' band. */
    aboveNavBar: Boolean = false,
    /** Whether the sheet always takes its full height (its band then stays at the bottom). */
    fill: Boolean = false,
    content: @Composable ColumnScope.() -> Unit,
) {
    val own = remember { MutableTransitionState(false) }
    LaunchedEffect(own) { own.targetState = true }
    val visible = LocalSheetVisible.current ?: own
    BackHandler(enabled = visible.targetState, onBack = onDismiss)
    val scope = rememberCoroutineScope()
    var drag by remember { mutableFloatStateOf(0f) }
    var height by remember { mutableIntStateOf(0) }
    val closeLabel = stringResource(R.string.ui_close_sheet)
    Box(Modifier.fillMaxSize()) {
        AnimatedVisibility(visible, enter = fadeIn(tween(PdfMotion.SHEET_IN_MS)), exit = fadeOut(tween(PdfMotion.SHEET_OUT_MS))) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(if (dim) pdfColors.scrim else Color.Transparent)
                    .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onDismiss)
                    .semantics { contentDescription = closeLabel },
            )
        }
        val density = LocalDensity.current
        val divider = pdfColors.divider
        val navBand = pdfColors.navBand
        androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
            val navBar = with(density) { WindowInsets.navigationBars.getBottom(density).toDp() }
            val maxH = minOf(if (aboveNavBar) maxHeight * maxHeightFraction + navBar else maxHeight * maxHeightFraction, maxHeight - topReserve)
            AnimatedVisibility(
                visible,
                modifier = Modifier.align(Alignment.BottomCenter),
                enter = slideInVertically(tween(PdfMotion.SHEET_IN_MS, easing = LinearOutSlowInEasing)) { it },
                exit = slideOutVertically(tween(PdfMotion.SHEET_OUT_MS, easing = FastOutLinearInEasing)) { it },
            ) {
                Column(
                    modifier
                        .fillMaxWidth()
                        .heightIn(max = maxH)
                        .offset { IntOffset(0, drag.roundToInt()) }
                        .onSizeChanged { height = it.height }
                        .background(background ?: pdfColors.surface)
                        .drawBehind { drawRect(divider, size = androidx.compose.ui.geometry.Size(size.width, 1.dp.toPx())) }
                        .pointerInput(Unit) { detectTapGestures() },
                ) {
                    // the handle: drag it (or the sheet's top) down to close
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(handleHeight)
                            .pointerInput(Unit) {
                                detectVerticalDragGestures(
                                    onDragEnd = {
                                        scope.launch {
                                            if (drag > height / 4f) {
                                                onDismiss()
                                            }
                                            animate(drag, 0f, animationSpec = tween(150)) { v, _ -> drag = v }
                                        }
                                    },
                                    onVerticalDrag = { _, dy -> drag = (drag + dy).coerceAtLeast(0f) },
                                )
                            },
                        contentAlignment = if (handleHeight < 24.dp) Alignment.TopCenter else Alignment.Center,
                    ) {
                        // 8 dp down in a short strip, as the comment panel has it
                        Box(Modifier.padding(top = if (handleHeight < 24.dp) 8.dp else 0.dp).width(42.dp).height(4.dp).clip(CircleShape).background(pdfColors.handle))
                    }
                    Column(Modifier.weight(1f, fill = fill)) { content() }
                    // the band behind the navigation buttons, in the viewer's colour
                    if (bottomBand) Box(Modifier.fillMaxWidth().windowInsetsBottomHeight(WindowInsets.navigationBars).background(navBand))
                }
            }
            with(density) { Unit }
        }
    }
}

/** A row of a sheet: icon, label, then [trailing] (a chevron for drill-in rows). 48 dp tall. */
@Composable
fun SheetRow(
    icon: ImageVector?,
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    chevron: Boolean = false,
    checked: Boolean = false,
    enabled: Boolean = true,
    tint: Color? = null,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    val c = pdfColors
    val color = tint ?: if (checked) c.accent else c.text
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, null, tint = if (enabled) (tint ?: if (checked) c.accent else c.icon) else c.icon.copy(alpha = 0.38f), modifier = Modifier.size(24.dp))
            Spacer(Modifier.width(9.dp))
        }
        Text(
            label,
            color = if (enabled) color else color.copy(alpha = 0.38f),
            // the reference sets these at 17 sp in a narrower face; 16 sp of Inter matches their width
            fontSize = 16.sp,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f).padding(vertical = 12.5.dp),
        )
        if (trailing != null) trailing()
        if (checked) Icon(LucideIcons.Check, null, tint = c.accent, modifier = Modifier.size(24.dp))
        if (chevron) Icon(LucideIcons.ChevronRight, null, tint = c.icon, modifier = Modifier.size(24.dp))
    }
}

/** A sub-sheet's header: a back chevron and a bold title. */
@Composable
fun SheetBackHeader(title: String, onBack: () -> Unit, trailing: (@Composable RowScope.() -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        BarIcon(LucideIcons.ChevronLeft, stringResource(R.string.common_back), onBack)
        Spacer(Modifier.width(6.dp))
        Text(title, color = pdfColors.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (trailing != null) trailing()
    }
}

/** A sheet's title line. */
@Composable
fun SheetTitle(title: String) {
    Text(title, color = pdfColors.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 4.dp, bottom = 12.dp))
}

/** A full-width divider line. */
@Composable
fun SheetDivider(inset: Dp = 16.dp, vertical: Dp = 0.dp) {
    Box(Modifier.padding(horizontal = inset, vertical = vertical).fillMaxWidth().height(1.dp).background(pdfColors.divider))
}

/** A divider between groups of rows: the reference leaves 4 dp more either side than a row's own padding. */
@Composable
fun GroupDivider() = SheetDivider(vertical = 4.dp)

/** The hint the reference shows over the page when a tool starts ("Drag across the text to highlight it"). */
@Composable
fun BoxScope.HintToast(text: String?, modifier: Modifier = Modifier, alignment: Alignment = Alignment.Center, corner: androidx.compose.ui.unit.Dp = 4.dp) {
    androidx.compose.animation.AnimatedVisibility(
        visible = text != null,
        modifier = modifier.align(alignment),
        enter = fadeIn(tween(150)),
        exit = fadeOut(tween(250)),
    ) {
        val last = remember { Last<String>() }
        if (text != null) last.value = text
        Text(
            last.value ?: "",
            color = pdfColors.onToast,
            fontSize = 15.sp,
            lineHeight = 20.sp,
            // the reference's box: 32 dp tall, 11 dp either side of the text
            modifier = Modifier.clip(RoundedCornerShape(corner)).background(pdfColors.toast).padding(horizontal = 11.dp, vertical = 6.dp),
        )
    }
}

/** A switch drawn as the reference's: a pill track with a round thumb. */
@Composable
fun PdfSwitch(checked: Boolean, onChange: (Boolean) -> Unit, modifier: Modifier = Modifier) {
    val c = pdfColors
    val track = if (checked) c.accent else if (c.dark) Color(0xFF959595) else Color(0xFFB0B0B0)
    Box(
        modifier
            .size(width = 36.dp, height = 18.dp)
            .clip(CircleShape)
            .background(if (checked) track else Color.Transparent)
            .then(if (checked) Modifier else Modifier.background(Color.Transparent))
            .clickable(role = Role.Switch) { onChange(!checked) },
    ) {
        if (!checked) {
            Box(Modifier.fillMaxSize().clip(CircleShape).background(track.copy(alpha = 0.0f)))
            androidx.compose.foundation.Canvas(Modifier.fillMaxSize()) {
                drawRoundRect(track, cornerRadius = androidx.compose.ui.geometry.CornerRadius(size.height / 2), style = androidx.compose.ui.graphics.drawscope.Stroke(2.dp.toPx()))
            }
        }
        Box(
            Modifier
                .align(if (checked) Alignment.CenterEnd else Alignment.CenterStart)
                .padding(horizontal = 4.dp)
                .size(10.dp)
                .clip(CircleShape)
                .background(if (checked) c.surface else track),
        )
    }
}

/**
 * Top padding for a full screen: the status bar, or the camera cutout while the viewer keeps the
 * status bar hidden.
 */
fun Modifier.safeTopPadding(): Modifier = composed {
    windowInsetsPadding(WindowInsets.statusBars.union(WindowInsets.displayCutout).only(WindowInsetsSides.Top))
}

/**
 * A dialog as the reference draws them: a card 51 dp in from the sides with a bold title, the
 * content, then an outlined pill (Cancel) and a filled blue pill (OK) at the right. The screen
 * behind is veiled (lighter in dark mode, darker in light mode).
 */
@Composable
fun PdfDialog(
    onDismiss: () -> Unit,
    title: String?,
    confirm: String?,
    onConfirm: () -> Unit = {},
    confirmEnabled: Boolean = true,
    dismiss: String? = stringResource(R.string.common_cancel),
    onDismissButton: () -> Unit = onDismiss,
    destructive: Boolean = false,
    /** A choice list's card: lighter grey, narrower, no padding of its own. */
    list: Boolean = false,
    content: (@Composable ColumnScope.() -> Unit)? = null,
) {
    val c = pdfColors
    androidx.compose.ui.window.Dialog(
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
    ) {
        // the screen behind is shaded 60 % black, as the reference's dialogs leave it
        val window = (androidx.compose.ui.platform.LocalView.current.parent as? androidx.compose.ui.window.DialogWindowProvider)?.window
        androidx.compose.runtime.SideEffect { window?.setDimAmount(0.6f) }
        Box(Modifier.fillMaxWidth().padding(horizontal = if (list) 68.dp else 51.dp), contentAlignment = Alignment.Center) {
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(if (list) (if (c.dark) Color(0xFF424242) else Color.White) else c.surface)
                    .padding(if (list) PaddingValues(vertical = 8.dp) else PaddingValues(24.dp)),
            ) {
                if (title != null) {
                    Text(title, color = c.label, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.height(10.dp))
                }
                content?.invoke(this)
                if (confirm != null || dismiss != null) {
                    Spacer(Modifier.height(20.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(18.dp, Alignment.End)) {
                        if (dismiss != null) DialogPill(dismiss, filled = false, enabled = true, onClick = onDismissButton)
                        if (confirm != null) DialogPill(confirm, filled = true, enabled = confirmEnabled, red = destructive, onClick = onConfirm)
                    }
                }
            }
        }
    }
}

@Composable
private fun DialogPill(label: String, filled: Boolean, enabled: Boolean, red: Boolean = false, onClick: () -> Unit) {
    val c = pdfColors
    val fill = if (red) c.red else Color(0xFF1473E6)
    Box(
        Modifier.heightIn(min = 42.dp).widthIn(min = 88.dp).clip(RoundedCornerShape(21.dp))
            .then(if (filled) Modifier.background(if (enabled) fill else c.pressed) else Modifier.border(2.dp, c.textSecondary, RoundedCornerShape(21.dp)))
            .clickable(enabled = enabled, onClick = onClick).padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (filled) (if (enabled) Color.White else c.textSecondary) else c.label, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

/** A dialog's text field: 48 dp tall, outlined (2 dp blue while focused, red when wrong), its hint in italics. */
@Composable
fun PdfField(
    value: String,
    onValue: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    error: Boolean = false,
    password: Boolean = false,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    keyboardActions: androidx.compose.foundation.text.KeyboardActions = androidx.compose.foundation.text.KeyboardActions.Default,
    focus: androidx.compose.ui.focus.FocusRequester? = null,
) {
    val c = pdfColors
    var focused by remember { mutableStateOf(false) }
    var visible by remember { mutableStateOf(false) }
    Row(
        modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(4.dp))
            .border(if (focused || error) 2.dp else 1.dp, if (error) c.red else if (focused) Color(0xFF378EF0) else c.textSecondary, RoundedCornerShape(4.dp))
            .padding(start = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            if (value.isEmpty()) Text(placeholder, color = c.textSecondary, fontSize = 16.sp, fontStyle = androidx.compose.ui.text.font.FontStyle.Italic)
            androidx.compose.foundation.text.BasicTextField(
                value = value, onValueChange = onValue, singleLine = true,
                textStyle = androidx.compose.ui.text.TextStyle(color = c.text, fontSize = 16.sp),
                cursorBrush = androidx.compose.ui.graphics.SolidColor(Color(0xFF378EF0)),
                visualTransformation = if (password && !visible) androidx.compose.ui.text.input.PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
                keyboardOptions = keyboardOptions, keyboardActions = keyboardActions,
                modifier = Modifier.fillMaxWidth().then(if (focus != null) Modifier.focusRequester(focus) else Modifier)
                    .onFocusChanged { focused = it.isFocused }.semantics { contentDescription = placeholder },
            )
        }
        if (password) BarIcon(if (visible) LucideIcons.Eye else LucideIcons.EyeOff, stringResource(if (visible) R.string.ui_hide_password else R.string.ui_show_password), { visible = !visible }, size = 48.dp)
        else Spacer(Modifier.width(12.dp))
    }
}

/**
 * A short label under an icon (toolbars, tool tiles), centred, on up to [maxLines] lines that
 * break only between words. A word too wide for the room (a large font size setting) makes the
 * whole label a little smaller, down to [minSize], rather than being split: "Organize pages"
 * read "Organi / ze pa…" at the largest setting.
 */
@Composable
fun WordWrapLabel(
    text: String,
    color: Color,
    size: androidx.compose.ui.unit.TextUnit,
    lineHeight: androidx.compose.ui.unit.TextUnit,
    modifier: Modifier = Modifier,
    maxLines: Int = 2,
    letterSpacing: androidx.compose.ui.unit.TextUnit = androidx.compose.ui.unit.TextUnit.Unspecified,
    minSize: androidx.compose.ui.unit.TextUnit = 7.sp,
) {
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    val style = androidx.compose.material3.LocalTextStyle.current.merge(
        androidx.compose.ui.text.TextStyle(
            color = color, fontSize = size, lineHeight = lineHeight, letterSpacing = letterSpacing,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        ),
    )
    androidx.compose.foundation.layout.BoxWithConstraints(modifier, contentAlignment = Alignment.TopCenter) {
        val room = constraints.maxWidth
        val scale = remember(text, style, room) {
            val widest = text.split(' ').filter { it.isNotBlank() }
                .maxOfOrNull { measurer.measure(it, style, softWrap = false, maxLines = 1).size.width } ?: 0
            // a little under the exact ratio, so the narrowed word doesn't land a pixel too wide
            if (room == androidx.compose.ui.unit.Constraints.Infinity || widest <= room || widest == 0) 1f
            else (room.toFloat() / widest * 0.97f).coerceAtLeast(minSize.value / size.value)
        }
        Text(text, style = style.copy(fontSize = size * scale, lineHeight = lineHeight * scale), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
    }
}
