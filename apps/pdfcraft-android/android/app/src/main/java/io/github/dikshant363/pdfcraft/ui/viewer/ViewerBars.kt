package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.BarDivider
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.Inter
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.flow.first

/** The viewer's top bar height (the reference's: 56 dp). */
val TopBarHeight = 57.dp

/** The bottom quick toolbar's height (the reference's: 70 dp). */
val BottomBarHeight = 70.dp

/**
 * The top bar in plain viewing: back, a divider and the Read aloud switch, then Find, Share and
 * More. With a tool active the back arrow becomes a check mark (Done), the switch goes, and Undo
 * and Redo appear once there is something to undo.
 */
@Composable
fun ViewerTopBar(
    toolActive: Boolean,
    doneIsCheck: Boolean,
    canUndo: Boolean,
    canRedo: Boolean,
    onBack: () -> Unit,
    onUndo: () -> Unit,
    onRedo: () -> Unit,
    readingAloud: Boolean,
    onReadAloud: () -> Unit,
    onSearch: (() -> Unit)?,
    onShare: () -> Unit,
    onMore: () -> Unit,
    moreOpen: Boolean,
    /** A tool with its own title (Edit PDF): Done, the title and Undo only. */
    title: String? = null,
    /** What Undo and Redo would take back or bring back, for their menu. */
    undoLabel: String? = null,
    redoLabel: String? = null,
) {
    val c = pdfColors
    Row(
        Modifier.fillMaxWidth().height(TopBarHeight).background(c.surface).drawBehind {
            val line = 1.dp.toPx()
            drawRect(c.divider, Offset(0f, size.height - line), androidx.compose.ui.geometry.Size(size.width, line))
        }.padding(bottom = 1.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(56.dp), contentAlignment = Alignment.Center) {
            if (toolActive && doneIsCheck) {
                BarIcon(LucideIcons.Check, stringResource(R.string.common_done), onBack, tint = c.toolAccent)
            } else {
                BarIcon(LucideIcons.ArrowLeft, stringResource(R.string.common_back), onBack, iconSize = 27.dp)
            }
        }
        if (title != null) {
            Spacer(Modifier.width(6.dp))
            Text(title, color = c.label, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, modifier = Modifier.weight(1f))
            UndoMenuButton(canUndo, canRedo, onUndo, onRedo, undoLabel, redoLabel)
            Spacer(Modifier.width(10.dp))
            return@Row
        }
        if (!toolActive) {
            BarDivider(height = 24.dp)
            Spacer(Modifier.width(14.dp))
            // with something to undo the reference shrinks the switch to a plain speaker button
            if (canUndo && !readingAloud) BarIcon(LucideIcons.Volume2, stringResource(R.string.common_tool_read_aloud), onReadAloud) else ReadAloudSwitch(readingAloud, onReadAloud)
        }
        Spacer(Modifier.weight(1f))
        // while reading aloud the bar holds only back and the switch, as the reference's
        if (readingAloud && !toolActive) {
            Spacer(Modifier.width(10.dp))
            return@Row
        }
        if (toolActive) {
            if (canUndo || canRedo) {
                BarIcon(LucideIcons.Undo2, stringResource(R.string.common_undo), onUndo, enabled = canUndo)
                BarIcon(LucideIcons.Redo2, stringResource(R.string.common_redo), onRedo, enabled = canRedo)
            }
        } else if (canUndo) {
            UndoMenuButton(canUndo, canRedo, onUndo, onRedo, undoLabel, redoLabel)
        }
        if (onSearch != null) BarIcon(LucideIcons.Search, stringResource(R.string.viewer_find_in_document), onSearch)
        BarIcon(LucideIcons.Share2, stringResource(R.string.common_share), onShare)
        BarIcon(LucideIcons.EllipsisVertical, stringResource(R.string.common_more_options), onMore, selected = moreOpen)
        Spacer(Modifier.width(10.dp))
    }
}

/** Undo with a corner mark: a tap opens Undo and Redo. */
@Composable
fun UndoMenuButton(canUndo: Boolean, canRedo: Boolean, onUndo: () -> Unit, onRedo: () -> Unit, undoLabel: String? = null, redoLabel: String? = null) {
    val c = pdfColors
    var open by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val undoOrRedo = stringResource(R.string.viewer_undo_or_redo)
    Box {
        Box(
            Modifier.size(46.dp).clickable(enabled = canUndo || canRedo, role = Role.Button) { open = true }.semantics { contentDescription = undoOrRedo },
            contentAlignment = Alignment.Center,
        ) {
            val tint = if (canUndo || canRedo) c.icon else c.icon.copy(alpha = 0.38f)
            Icon(LucideIcons.Undo2, null, tint = tint, modifier = Modifier.size(24.dp))
            Box(Modifier.size(6.dp).offset(x = 12.dp, y = 10.dp).drawBehind {
                val p = Path().apply { moveTo(size.width, 0f); lineTo(size.width, size.height); lineTo(0f, size.height); close() }
                drawPath(p, tint)
            })
        }
        androidx.compose.material3.DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = c.menu) {
            androidx.compose.material3.DropdownMenuItem(
                text = { Text(undoLabel?.let { stringResource(R.string.viewer_undo_named, it) } ?: stringResource(R.string.common_undo), color = c.text) }, enabled = canUndo,
                leadingIcon = { Icon(LucideIcons.Undo2, null, tint = c.icon) }, onClick = { open = false; onUndo() },
            )
            androidx.compose.material3.DropdownMenuItem(
                text = { Text(redoLabel?.let { stringResource(R.string.viewer_redo_named, it) } ?: stringResource(R.string.common_redo), color = c.text) }, enabled = canRedo,
                leadingIcon = { Icon(LucideIcons.Redo2, null, tint = c.icon) }, onClick = { open = false; onRedo() },
            )
        }
    }
}

/** Read aloud as the reference shows it: a switch whose knob carries a speaker (crossed out while off). */
@Composable
private fun ReadAloudSwitch(on: Boolean, onToggle: () -> Unit) {
    val c = pdfColors
    val knob by androidx.compose.animation.core.animateDpAsState(if (on) 22.dp else 2.dp, label = "knob")
    val track by androidx.compose.animation.animateColorAsState(if (on) c.switchOn else c.switchOff, label = "track")
    val readAloud = stringResource(R.string.common_tool_read_aloud)
    Box(
        Modifier.size(width = 56.dp, height = 40.dp).clickable(role = Role.Switch, onClick = onToggle).semantics { contentDescription = readAloud },
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(Modifier.size(width = 56.dp, height = 28.dp).background(track, androidx.compose.foundation.shape.RoundedCornerShape(14.dp))) {
            Box(
                Modifier.offset(x = knob, y = 2.dp).size(width = 32.dp, height = 24.dp).background(Color.White, androidx.compose.foundation.shape.RoundedCornerShape(12.dp)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(if (on) LucideIcons.Volume2 else LucideIcons.VolumeOff, null, tint = Color(0xFF222222), modifier = Modifier.size(16.dp))
            }
        }
    }
}

/**
 * Find in the top bar: collapse, the query field (searched on submit), then clear, Previous and
 * Next, greyed until they can be used. Like the reference, it shows no hit counter, unless Extra
 * Features asks for one: [count], "3 of 12", between the field and the buttons.
 */
@Composable
fun SearchBar(
    query: String,
    onQuery: (String) -> Unit,
    canStep: Boolean,
    onSubmit: () -> Unit,
    onClear: () -> Unit,
    onClose: () -> Unit,
    onPrev: () -> Unit,
    onNext: () -> Unit,
    count: String? = null,
) {
    val c = pdfColors
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val findLabel = stringResource(R.string.viewer_find_in_document)
    LaunchedEffect(Unit) { focus.requestFocus() }
    Row(
        Modifier.fillMaxWidth().height(TopBarHeight).background(c.surface).drawBehind {
            val line = 1.dp.toPx()
            drawRect(c.divider, Offset(0f, size.height - line), androidx.compose.ui.geometry.Size(size.width, line))
        }.padding(bottom = 1.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(56.dp), contentAlignment = Alignment.Center) { BarIcon(LucideIcons.ArrowLeft, stringResource(R.string.viewer_collapse), onClose, iconSize = 27.dp) }
        Box(
            Modifier.weight(1f).padding(horizontal = 8.dp).height(40.dp).drawBehind {
                drawLine(c.textSecondary, Offset(0f, size.height - 1f), Offset(size.width, size.height - 1f), 1.5f)
            },
            contentAlignment = Alignment.CenterStart,
        ) {
            BasicTextField(
                value = query,
                onValueChange = onQuery,
                singleLine = true,
                textStyle = TextStyle(fontFamily = Inter, fontSize = 17.sp, color = c.text),
                cursorBrush = SolidColor(c.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = {
                    keyboard?.hide()
                    onSubmit()
                }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = findLabel },
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (query.isEmpty()) Text(findLabel, color = c.textSecondary, fontSize = 17.sp, fontStyle = FontStyle.Italic)
                        inner()
                    }
                },
            )
        }
        if (count != null) Text(count, color = c.textSecondary, fontSize = 15.sp, maxLines = 1, modifier = Modifier.padding(horizontal = 4.dp))
        BarIcon(LucideIcons.X, stringResource(R.string.common_clear), onClear, size = 40.dp, enabled = query.isNotEmpty())
        BarIcon(LucideIcons.ChevronLeft, stringResource(R.string.viewer_find_previous), onPrev, enabled = canStep, size = 40.dp)
        BarIcon(LucideIcons.ChevronRight, stringResource(R.string.viewer_find_next), onNext, enabled = canStep, size = 40.dp)
        Spacer(Modifier.width(6.dp))
    }
}

/**
 * An item of the bottom quick toolbar, laid out as the reference does: 5 dp down, the icon in a
 * 44 dp box (shaded while its tool is open), the label straight under it, and a corner mark when
 * the tool has sub-tools. As the reference's, the label follows the phone's font size setting: the
 * item grows taller to hold it, and a long label wraps onto a second line (in a fixed height, a
 * large font setting cut the labels off at the bottom).
 */
@Composable
private fun QuickItem(icon: ImageVector, label: String, onClick: () -> Unit, subTools: Boolean = false, selected: Boolean = false, width: Dp) {
    val c = pdfColors
    Column(
        Modifier.width(width).heightIn(min = BottomBarHeight).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = label }
            .padding(horizontal = 2.dp, vertical = 5.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier.size(44.dp).offset(y = 2.5.dp).background(if (selected) c.divider else Color.Transparent, androidx.compose.foundation.shape.RoundedCornerShape(4.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, null, tint = c.icon, modifier = Modifier.size(24.dp))
            if (subTools) {
                // the small corner triangle: this tool has a row of sub-tools
                val tint = c.cornerMark
                Box(Modifier.size(6.dp).offset(x = 16.dp, y = 13.dp).drawBehind {
                    val p = Path().apply { moveTo(size.width, 0f); lineTo(size.width, size.height); lineTo(0f, size.height); close() }
                    drawPath(p, tint)
                })
            }
        }
        // the reference's 11 sp face is narrower than Inter: a half point less gives its height, and
        // tighter letters its width; lines close together, so a wrapped label stays one block, and
        // a word too wide for its slot (a very large font setting) shrinks rather than splits
        io.github.dikshant363.pdfcraft.ui.WordWrapLabel(
            label, c.label, size = 10.5.sp, lineHeight = 13.sp, letterSpacing = (-0.5).sp,
            modifier = Modifier.padding(top = 2.5.dp),
        )
    }
}

/** The strip above the quick toolbar's tools that holds its handle. */
val QuickHandleHeight = 22.dp

/** The quick toolbar's height with its handle, the shortcuts folded away. */
val QuickBarHeight = BottomBarHeight + QuickHandleHeight

/** A shortcut in the row the quick toolbar's handle pulls up. */
class QuickChip(val label: String, val icon: ImageVector, val tint: Color, val onClick: () -> Unit)

/**
 * The quick toolbar: a handle, then the tools the user keeps there (the reference's six at first)
 * and More tools. Pulling the handle up (or tapping it) shows a row of [chips] above the tools,
 * scrolling sideways; pulling it down folds them away. No line along its top, as the reference.
 */
@Composable
fun QuickToolbar(
    tools: List<QuickTool>,
    onTool: (QuickTool) -> Unit,
    onMoreTools: () -> Unit,
    moreToolsOpen: Boolean,
    chips: List<QuickChip> = emptyList(),
    /** How much taller than usual the tools' row is (a large font setting), in pixels. */
    onExtraHeight: (Int) -> Unit = {},
) {
    val c = pdfColors
    var open by remember { androidx.compose.runtime.mutableStateOf(false) }
    val handleLabel = stringResource(if (open) R.string.viewer_hide_shortcuts else R.string.viewer_show_shortcuts)
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth().background(c.surface)) {
        val slots = toolbarSlots(androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp)
        val shown = tools.take(slots)
        val itemW = minOf(72.dp, (maxWidth - 16.dp) / maxOf(7, shown.size + 1))
        Column(
            Modifier.fillMaxWidth().pointerInput(chips.isNotEmpty()) {
                if (chips.isEmpty()) return@pointerInput
                var pulled = 0f
                detectVerticalDragGestures(
                    onDragStart = { pulled = 0f },
                    onDragEnd = { if (pulled < -8f) open = true else if (pulled > 8f) open = false },
                ) { _, dy -> pulled += dy }
            },
        ) {
            Box(
                Modifier.fillMaxWidth().height(QuickHandleHeight)
                    .clickable(enabled = chips.isNotEmpty(), role = Role.Button) { open = !open }
                    .semantics { contentDescription = handleLabel },
                contentAlignment = Alignment.TopCenter,
            ) {
                // measured on the dark bar; the light bar's grey is our own match
                val handle = if (c.dark) Color(0xFF6A6A6A) else Color(0xFFC4C4C4)
                Box(Modifier.padding(top = 15.dp).size(40.dp, 4.dp).background(handle, androidx.compose.foundation.shape.RoundedCornerShape(2.dp)))
            }
            androidx.compose.animation.AnimatedVisibility(open && chips.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(androidx.compose.foundation.rememberScrollState())
                        .padding(start = 18.dp, end = 18.dp, top = 5.dp, bottom = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(11.dp),
                ) { for (chip in chips) QuickChipView(chip) }
            }
            val usual = with(androidx.compose.ui.platform.LocalDensity.current) { BottomBarHeight.roundToPx() }
            Row(
                Modifier.fillMaxWidth().heightIn(min = BottomBarHeight)
                    .onSizeChanged { onExtraHeight((it.height - usual).coerceAtLeast(0)) },
                horizontalArrangement = Arrangement.Center,
            ) {
                for (t in shown) QuickItem(t.icon, stringResource(t.label), { onTool(t) }, subTools = t.subTools, width = itemW)
                QuickItem(LucideIcons.LayoutGrid, stringResource(R.string.common_tool_more_tools), onMoreTools, selected = moreToolsOpen, width = itemW)
            }
        }
    }
}

/** One shortcut: its coloured icon and name on a dark rounded box, 37 dp tall. */
@Composable
private fun QuickChipView(chip: QuickChip) {
    val c = pdfColors
    val shape = androidx.compose.foundation.shape.RoundedCornerShape(6.dp)
    Row(
        Modifier.heightIn(min = 37.dp).clip(shape).background(if (c.dark) Color(0xFF080808) else Color(0xFFEDEDED))
            .clickable(role = Role.Button, onClick = chip.onClick).padding(start = 7.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // sized against the reference: its icon draws about 18 dp, its name 14 sp in a narrower face
        Icon(chip.icon, null, tint = chip.tint, modifier = Modifier.size(21.dp))
        Spacer(Modifier.width(8.dp))
        Text(chip.label, color = c.text, fontSize = 14.sp, letterSpacing = (-0.65).sp, maxLines = 1)
    }
}

/** The 1 dp line the reference draws along the top of its bottom bars, just above them. */
@Composable
fun Modifier.barTopLine(): Modifier {
    val color = pdfColors.divider
    return drawBehind {
        val line = 1.dp.toPx()
        drawRect(color, Offset(0f, -line), androidx.compose.ui.geometry.Size(size.width, line))
    }
}

/**
 * A tool's sub-toolbar: its buttons centred on the bar, groups split by thin lines. As the reference:
 * the buttons keep their size, and when they don't all fit (a narrow screen, a large display size)
 * the bar scrolls sideways, sliding to a tool when it's chosen. (Squeezing the buttons together to
 * fit was tried and left out; see docs/parity-mobile.md, "Tool bars on narrow screens".)
 */
@Composable
fun SubToolbar(content: @Composable () -> Unit) {
    val scroll = androidx.compose.foundation.rememberScrollState()
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth().height(BottomBarHeight).barTopLine().background(pdfColors.surface)) {
        Row(
            Modifier.fillMaxHeight().horizontalScroll(scroll).widthIn(min = maxWidth),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) { androidx.compose.runtime.CompositionLocalProvider(LocalSubToolbarScroll provides scroll) { content() } }
    }
}

/** The scrolling bar a sub-toolbar tool sits in (none outside one). */
private val LocalSubToolbarScroll = androidx.compose.runtime.staticCompositionLocalOf<androidx.compose.foundation.ScrollState?> { null }

/** When the tool becomes the chosen one, the bar scrolls to bring it to the start (as far as it goes). */
@Composable
private fun Modifier.slideToWhenChosen(selected: Boolean): Modifier {
    val scroll = LocalSubToolbarScroll.current ?: return this
    var x by remember { androidx.compose.runtime.mutableStateOf<Int?>(null) }
    LaunchedEffect(selected) {
        if (!selected) return@LaunchedEffect
        // once the tool has been placed on the bar
        val at = androidx.compose.runtime.snapshotFlow { x }.first { it != null } ?: return@LaunchedEffect
        scroll.animateScrollTo(at)
    }
    return onPlaced { x = it.positionInParent().x.toInt() }
}

/** A sub-toolbar tool: 62 dp apart, as the reference spaces them. */
@Composable
fun SubTool(icon: ImageVector, description: String, selected: Boolean, onClick: () -> Unit, enabled: Boolean = true) {
    SubButton(icon, description, selected, onClick, enabled, width = 62.dp)
}

/** Fill & Sign's text tool: an "A" in a dashed box (a field to type in). */
@Composable
fun SubTextField(description: String, selected: Boolean, onClick: () -> Unit) {
    val tint = if (selected) pdfColors.toolAccent else pdfColors.icon
    Box(
        Modifier.width(62.dp).height(54.dp).slideToWhenChosen(selected).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Box(
            Modifier.size(22.dp).drawBehind {
                val w = 1.5.dp.toPx()
                drawRect(
                    tint, topLeft = Offset(w / 2, w / 2), size = androidx.compose.ui.geometry.Size(size.width - w, size.height - w),
                    style = androidx.compose.ui.graphics.drawscope.Stroke(w, pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(2.5.dp.toPx(), 2.dp.toPx()))),
                )
            },
            contentAlignment = Alignment.Center,
        ) { Text("A", color = tint, fontSize = 14.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold) }
    }
}

/** A sub-toolbar tool drawn as a filled dot (Fill & Sign's dot mark). */
@Composable
fun SubDot(description: String, selected: Boolean, onClick: () -> Unit) {
    val tint = if (selected) pdfColors.toolAccent else pdfColors.icon
    Box(
        Modifier.width(62.dp).height(54.dp).slideToWhenChosen(selected).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { Box(Modifier.size(8.dp).background(tint, androidx.compose.foundation.shape.CircleShape)) }
}

/** A sub-toolbar property (line thickness, text size): narrower than a tool, beside its colour; shaded while its picker is open. */
@Composable
fun SubProperty(icon: ImageVector, description: String, selected: Boolean, onClick: () -> Unit) {
    SubButton(icon, description, selected, onClick, true, width = 36.dp, shaded = true)
}

@Composable
private fun SubButton(icon: ImageVector, description: String, selected: Boolean, onClick: () -> Unit, enabled: Boolean, width: Dp, shaded: Boolean = false) {
    val c = pdfColors
    val tint = if (selected && !shaded) c.toolAccent else c.icon
    Box(
        Modifier.width(width).height(54.dp).slideToWhenChosen(selected && !shaded).clickable(enabled = enabled, role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        if (shaded && selected) Box(Modifier.requiredSize(40.dp).background(c.pressed, androidx.compose.foundation.shape.RoundedCornerShape(5.dp)))
        Icon(icon, null, tint = if (enabled) tint else tint.copy(alpha = 0.38f), modifier = Modifier.size(24.dp))
    }
}

/** A sub-toolbar divider with the reference's spacing. */
@Composable
fun SubDivider() {
    Box(Modifier.padding(horizontal = 4.dp)) { BarDivider(height = 30.dp) }
}

/** The colour button of a sub-toolbar: a filled dot of the tool's colour. */
@Composable
fun ColorDot(color: androidx.compose.ui.graphics.Color, selected: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    val colourLabel = stringResource(R.string.viewer_colour)
    Box(
        Modifier.width(36.dp).height(54.dp).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = colourLabel },
        contentAlignment = Alignment.Center,
    ) {
        if (selected) Box(Modifier.requiredSize(40.dp).background(c.pressed, androidx.compose.foundation.shape.RoundedCornerShape(5.dp)))
        Box(Modifier.size(20.dp).background(color, androidx.compose.foundation.shape.CircleShape))
    }
}
