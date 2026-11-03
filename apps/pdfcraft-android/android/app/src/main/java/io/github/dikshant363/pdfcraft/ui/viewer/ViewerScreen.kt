package io.github.dikshant363.pdfcraft.ui.viewer

import android.app.Activity
import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.magnifier
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.layout.imePadding
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.BarIcon
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.clickable
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.HintToast
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfMotion
import io.github.dikshant363.pdfcraft.ui.SheetHost
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The document viewer: the pages, the top bar and the bottom quick toolbar (a tap on the page
 * hides and shows them, with the system bars), the scrubber and its page pill, Find, text
 * selection, the active tool's layer and sub-toolbar, and the sheets.
 */
@Composable
fun ViewerScreen(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val view = LocalView.current
    val context = LocalContext.current

    // The system bars go with the viewer's bars.
    val statusBar = vm.statusBarInDocuments
    DisposableEffect(ctl.barsVisible, statusBar, c.dark) {
        val window = (context as? Activity)?.window
        if (window != null) {
            val insets = WindowCompat.getInsetsController(window, view)
            insets.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            // the status bar stays hidden in the viewer (a black band where the camera is), as the
            // reference's, unless Extra Features shows it with the bars; the navigation bar comes and
            // goes with the viewer's bars
            if (statusBar && ctl.barsVisible) insets.show(WindowInsetsCompat.Type.statusBars()) else insets.hide(WindowInsetsCompat.Type.statusBars())
            if (ctl.barsVisible) insets.show(WindowInsetsCompat.Type.navigationBars()) else insets.hide(WindowInsetsCompat.Type.navigationBars())
            // light status icons on the black band, as the reference has in both themes; shown, they
            // sit on the top bar's colour
            insets.isAppearanceLightStatusBars = statusBar && !c.dark
            insets.isAppearanceLightNavigationBars = !c.dark
        }
        onDispose {
            if (window != null) WindowCompat.getInsetsController(window, view).show(WindowInsetsCompat.Type.systemBars())
        }
    }

    // The page area starts under the top bar while it shows, and leaves room for the bottom bar.
    val statusPx = maxOf(WindowInsets.statusBars.getTop(density), WindowInsets.displayCutout.getTop(density)).toFloat()
    val navPx = WindowInsets.navigationBars.getBottom(density).toFloat()
    val topPx = with(density) { TopBarHeight.toPx() }
    // the quick toolbar grows with a large font setting: the page area leaves room for that too
    var quickExtraPx by remember { mutableIntStateOf(0) }
    val bottomPx = with(density) { if (ctl.tool == ViewerTool.None) QuickBarHeight.toPx() + quickExtraPx else BottomBarHeight.toPx() }
    // keyed on the document too: one opened over another (Create, Combine) has a fresh page view
    LaunchedEffect(doc, ctl.barsVisible, statusPx, navPx, ctl.searching, ctl.tool, vm.readingMode, bottomPx, statusBar) {
        val bottomShown = ctl.barsVisible && !ctl.searching && !vm.readingMode
        doc.viewer.setPadding(
            // with the status bar in documents, the page goes up to the top edge once the bars hide
            top = if (ctl.barsVisible) statusPx + topPx else if (statusBar) 0f else statusPx,
            bottom = if (bottomShown) bottomPx + navPx else navPx,
        )
    }

    // Reopen where the reader left off.
    LaunchedEffect(doc) {
        snapshotFlow { doc.viewer.viewW }.collect { w ->
            if (w > 0f) vm.pendingPage?.let { p ->
                val spot = vm.pendingSpot
                if (spot != null) doc.viewer.showSpot(p, spot.first, spot.second, spot.third) else doc.viewer.goToPage(p)
                vm.pendingPage = null
                vm.pendingSpot = null
            }
        }
    }

    // Opening the document, scrolling and zooming show the scrubber and page pill for a while.
    LaunchedEffect(doc) {
        snapshotFlow { Triple(doc.viewer.scrollX, doc.viewer.scrollY, doc.viewer.zoom) }.collect {
            ctl.lastMoved = SystemClock.uptimeMillis()
        }
    }
    var indicatorVisible by remember { mutableStateOf(false) }
    LaunchedEffect(ctl.lastMoved) {
        if (ctl.lastMoved == 0L) return@LaunchedEffect
        indicatorVisible = true
        delay(ViewerController.INDICATOR_MS)
        indicatorVisible = false
    }

    // The hint a tool shows when it starts.
    var hint by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(ctl.tool, ctl.markup) {
        hint = toolHint(ctl.tool, ctl.markup, doc)?.takeIf { vm.toolPrefs.takeToolHint("${ctl.tool}.${if (ctl.tool == ViewerTool.Markup) ctl.markup else ""}") }
        if (hint != null) {
            delay(2500)
            hint = null
        }
    }

    val search = doc.search
    val selection = doc.selection
    // on a desktop, zoom buttons at the bottom right: with the bars, or for a moment when the mouse
    // comes near the corner or the wheel zooms, as the reference has them
    val config = androidx.compose.ui.platform.LocalConfiguration.current
    val desktop = remember(config) { isDesktop(context, config) }
    val zoomButtons = remember(doc) { ZoomButtonsState(scope) }
    LaunchedEffect(ctl.barsVisible) { if (!ctl.barsVisible) zoomButtons.hide() }
    val canvasLook = CanvasLook(background = c.desk, pageShadow = c.pageEdge, invert = ctl.nightMode)

    BackHandler(enabled = vm.fieldEdit != null || ctl.searching || ctl.tool != ViewerTool.None || selection.selection != null) {
        when {
            vm.fieldEdit != null -> vm.commitFieldEdit()
            selection.selection != null -> selection.clear()
            ctl.searching -> closeSearch(doc)
            else -> vm.leaveTool(doc)
        }
    }

    // a magnifier over the text while a selection handle is dragged, as the reference shows one
    Box(
        Modifier.fillMaxSize().background(c.desk).magnifier(
            sourceCenter = { selection.magnifierAt ?: Offset.Unspecified },
            magnifierCenter = { selection.magnifierAt?.let { it - Offset(0f, with(density) { 56.dp.toPx() }) } ?: Offset.Unspecified },
            zoom = 1.25f,
            size = androidx.compose.ui.unit.DpSize(120.dp, 44.dp),
            cornerRadius = 22.dp,
        ),
    ) {
        val gesturesForTool = ctl.tool == ViewerTool.None || ctl.tool == ViewerTool.Edit
        if (vm.readingMode) ReadingModeContent(
            vm, doc,
            // the text stays put when the bars hide, as the pages do
            top = with(density) { (statusPx + topPx).toDp() },
            bottom = with(density) { doc.viewer.padBottom.toDp() },
            onTap = { ctl.toggleBars() },
        ) else PageCanvas(
            state = doc.viewer,
            images = doc.images,
            generation = doc.info.generation.toLong(),
            look = canvasLook,
            gap = with(density) { (if (c.dark) 3f else 4f).toDp() },
            gesturesEnabled = gesturesForTool,
            onTap = { p, at ->
                when {
                    selection.selection != null -> selection.clear()
                    ctl.tool == ViewerTool.Edit -> vm.editTap(doc, at)
                    vm.tapField(doc, at) -> Unit
                    vm.tapComment(doc, at) -> Unit
                    vm.tapLink(doc, at) -> Unit
                    // while reading aloud, a word moves the reading there; anywhere else shows or hides the bars
                    vm.readAloud != null && at != null && ctl.tool == ViewerTool.None -> scope.launch {
                        val slop = 12f / doc.viewer.pixelsPerPoint(at.page).coerceAtLeast(0.1f) * density.density
                        if (!vm.readAloudAt(doc, at, slop)) ctl.toggleBars()
                    }
                    doc.viewer.mode == PageMode.SinglePage && doc.viewer.zoom <= 1.01f && edgeTap(p.x, doc.viewer.viewW) != 0 -> {
                        doc.viewer.goToPage(doc.viewer.currentPage + edgeTap(p.x, doc.viewer.viewW))
                        ctl.barsVisible = false
                    }
                    else -> ctl.toggleBars()
                }
            },
            // a double tap zooms with the bars out of the way, as the reference's does
            onDoubleTap = { if (ctl.tool == ViewerTool.None && !ctl.searching) ctl.barsVisible = false },
            // with no tool out, a mouse drag selects text, as a desktop does
            onMouseSelect = if (ctl.tool == ViewerTool.None) selection::selectSpan else null,
            onMouseHover = { p ->
                val v = doc.viewer
                val corner = with(density) { p.x > v.viewW - ZOOM_CORNER_WIDTH.toPx() && p.y > v.viewH - ZOOM_CORNER_HEIGHT.toPx() }
                if (desktop && corner) zoomButtons.poke()
            },
            onWheelZoom = { if (desktop) zoomButtons.poke() },
            onLongPress = { p, at ->
                if (at != null) scope.launch {
                    val slop = 12f / doc.viewer.pixelsPerPoint(at.page).coerceAtLeast(0.1f) * density.density
                    // no word there: a picture, or the page menu, as the reference's long press
                    if (!selection.selectWordAt(at, slop) && ctl.tool == ViewerTool.None && !vm.readingMode) vm.longPressPage(doc, at, p)
                }
            },
            overlay = {
                if (ctl.tool == ViewerTool.None || ctl.tool == ViewerTool.FillSign) drawFieldTints(doc)
                if (!doc.info.commentsHidden) drawNotes(doc, ctl.nightMode, vm.selectedComment?.takeIf { it.first === doc }?.second)
                vm.selectedComment?.takeIf { it.first === doc }?.second?.let { drawCommentSelection(doc, it) }
                vm.readAloud?.let { drawReadAloud(doc, it) }
                drawSearchHits(doc)
                drawSelection(doc)
                drawLinkFlash(doc)
            },
        )

        // The active tool takes the touches over the pages.
        ToolLayer(vm, doc)
        if (ctl.tool == ViewerTool.Edit) EditOverlay(vm, doc)
        vm.textPrompt?.takeIf { it.doc === doc }?.let { TextEntry(vm, it, onDone = vm::postText) }
        vm.fillTextPrompt?.takeIf { it.doc === doc }?.let { TextEntry(vm, it, onDone = vm::postFillText) }
        vm.addTextPrompt?.takeIf { it.doc === doc }?.let { TextEntry(vm, it, onDone = vm::postAddText) }
        vm.fieldEdit?.takeIf { it.doc === doc }?.let { FieldEditor(vm, it) }
        // a Fill & Sign element can be dragged and, for boxes, resized (a comment's: over its panel, below)
        if (ctl.tool == ViewerTool.FillSign) vm.selectedFill?.let { c ->
            CommentDragOverlay(doc, c, onMove = { dx, dy -> vm.moveFill(doc, c, dx, dy) }, onResize = { vm.resizeFill(doc, c, it) })
        }
        if (ctl.tool == ViewerTool.FillSign) vm.selectedFill?.let { FillElementMenu(vm, doc, it) }

        // Scrubber and page pill (continuous, three pages or more).
        if (vm.readingMode) {
            Unit
        } else if (doc.pageCount >= 3 && doc.viewer.mode == PageMode.SinglePage) {
            HorizontalScrubber(
                doc = doc,
                visible = indicatorVisible && !ctl.searching,
                bottom = with(density) { doc.viewer.padBottom.toDp() },
                onDrag = { ctl.lastMoved = SystemClock.uptimeMillis() },
                onTap = { ctl.goToPage = true },
            )
        } else if (doc.pageCount >= 3) {
            Scrubber(
                doc = doc,
                visible = indicatorVisible && !ctl.searching,
                top = with(density) { (doc.viewer.padTop).toDp() },
                bottom = with(density) { (doc.viewer.padBottom).toDp() },
                onDrag = { ctl.lastMoved = SystemClock.uptimeMillis() },
                onTap = { ctl.goToPage = true },
            )
        }

        if (desktop && !vm.readingMode) ZoomButtons(
            doc.viewer,
            visible = ctl.tool == ViewerTool.None && !ctl.searching && (ctl.barsVisible || zoomButtons.showing),
            // above the bottom bar when it shows
            bottom = maxOf(122.dp, with(density) { doc.viewer.padBottom.toDp() } + 12.dp),
            onHover = { if (!ctl.barsVisible) zoomButtons.poke() },
            modifier = Modifier.align(Alignment.BottomEnd),
        )

        HintToast(hint)

        // the file's name in a translucent white chip under the top bar for a moment after it opens
        // (80 % white: grey over the desk, near-white over a page, as the reference's)
        var nameChip by remember(doc) { mutableStateOf(false) }
        LaunchedEffect(doc) {
            delay(900)
            nameChip = true
            delay(2000)
            nameChip = false
        }
        androidx.compose.animation.AnimatedVisibility(
            nameChip && ctl.tool == ViewerTool.None,
            modifier = Modifier.align(Alignment.TopCenter).padding(top = with(density) { doc.viewer.padTop.toDp() } + 22.dp),
            enter = androidx.compose.animation.fadeIn(), exit = androidx.compose.animation.fadeOut(),
        ) {
            Row(
                Modifier.height(32.dp).clip(androidx.compose.foundation.shape.RoundedCornerShape(5.dp)).background(Color(0xCCFFFFFF))
                    .border(0.5.dp, Color(0xFFF5F5F5), androidx.compose.foundation.shape.RoundedCornerShape(5.dp))
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(doc.title.removeSuffix(".pdf"), color = Color(0xFF202020), fontSize = 13.sp, maxLines = 1)
                Spacer(Modifier.width(4.dp))
                Text("PDF", color = Color(0xFF202020), fontSize = 11.sp)
            }
        }

        // The bars.
        Column(Modifier.fillMaxWidth().align(Alignment.TopCenter)) {
            Box(
                Modifier.fillMaxWidth().height(with(density) { statusPx.toDp() })
                    .background(if (!statusBar) Color.Black else if (ctl.barsVisible) c.surface else Color.Transparent),
            )
            AnimatedVisibility(
                ctl.barsVisible,
                enter = slideInVertically(tween(PdfMotion.BARS_MS)) { -it } + fadeIn(tween(PdfMotion.BARS_MS)),
                exit = slideOutVertically(tween(PdfMotion.BARS_MS)) { -it } + fadeOut(tween(PdfMotion.BARS_MS)),
            ) {
                if (ctl.searching) {
                    var query by remember { mutableStateOf(search.query) }
                    SearchBar(
                        query = query,
                        onQuery = {
                            query = it
                            if (it.isEmpty()) search.clear()
                        },
                        canStep = search.hits.size > 1,
                        onSubmit = { runSearch(vm, doc, query.trim()) },
                        onClear = {
                            query = ""
                            search.clear()
                        },
                        onClose = { closeSearch(doc) },
                        onPrev = { search.step(false)?.let { showHit(doc, it) } },
                        onNext = { search.step(true)?.let { showHit(doc, it) } },
                        // Extra Features: which result is on screen, of how many (still growing while
                        // the rest of a long document is searched)
                        count = if (vm.searchCount && search.hits.isNotEmpty()) {
                            stringResource(R.string.viewer_search_count, (search.current + 1).coerceAtLeast(1), search.hits.size)
                        } else null,
                    )
                } else {
                    val fa = vm.fieldActions
                    if (fa != null) FieldTopBar(fa) else ViewerTopBar(
                        toolActive = ctl.tool != ViewerTool.None,
                        doneIsCheck = ctl.tool != ViewerTool.Comment,
                        // while drawing, the strokes not yet added have their own undo and redo
                        canUndo = doc.localUndo?.canUndo?.invoke() ?: (doc.info.undo != null),
                        canRedo = doc.localUndo?.canRedo?.invoke() ?: (doc.info.redo != null),
                        onBack = { if (ctl.tool != ViewerTool.None) vm.leaveTool(doc) else vm.closeDocument() },
                        onUndo = { vm.undo(doc) },
                        onRedo = { vm.redo(doc) },
                        readingAloud = vm.readAloud != null,
                        onReadAloud = { vm.readAloud?.stop() ?: vm.startReadAloud(doc) },
                        // Reading Mode has no search, as the reference's bar shows
                        onSearch = if (vm.readingMode) null else ({
                            selection.clear()
                            ctl.searching = true
                            ctl.sheet = ViewerSheet.None
                        }),
                        onShare = { vm.share(doc) },
                        onMore = { ctl.sheet = if (ctl.sheet == ViewerSheet.More) ViewerSheet.None else ViewerSheet.More },
                        moreOpen = ctl.sheet == ViewerSheet.More,
                        title = if (ctl.tool == ViewerTool.Edit) stringResource(R.string.common_tool_edit_pdf) else null,
                        undoLabel = if (doc.localUndo != null) stringResource(R.string.viewer_undo_drawing) else doc.info.undo,
                        redoLabel = if (doc.localUndo != null) stringResource(R.string.viewer_undo_drawing) else doc.info.redo,
                    )
                }
            }
        }
        // the Read Aloud player takes the bottom bar's place
        // while typing on the page, the bar (with its formatting) rides above the keyboard
        if (vm.readAloud == null && vm.fieldActions == null) Column(Modifier.fillMaxWidth().align(Alignment.BottomCenter).then(if (vm.editing != null) Modifier.imePadding() else Modifier)) {
            vm.success?.let { SuccessBannerView(it, onClose = { vm.success = null }) }
            AnimatedVisibility(
                ctl.barsVisible && !ctl.searching && !vm.readingMode,
                enter = slideInVertically(tween(PdfMotion.BARS_MS)) { it } + fadeIn(tween(PdfMotion.BARS_MS)),
                exit = slideOutVertically(tween(PdfMotion.BARS_MS)) { it } + fadeOut(tween(PdfMotion.BARS_MS)),
            ) {
                Column {
                    if (ctl.tool == ViewerTool.None) {
                        QuickToolbar(
                            tools = vm.quickToolbar.bar,
                            onTool = { vm.runQuickTool(doc, it) },
                            onMoreTools = { ctl.sheet = if (ctl.sheet == ViewerSheet.MoreTools) ViewerSheet.None else ViewerSheet.MoreTools },
                            moreToolsOpen = ctl.sheet == ViewerSheet.MoreTools,
                            chips = listOf(
                                QuickChip(stringResource(if (vm.isStarred(doc)) R.string.viewer_unstar_file else R.string.viewer_star_file), LucideIcons.Star, ToolHues.combine) { vm.toggleStar(doc) },
                                QuickChip(stringResource(R.string.common_tool_read_aloud), LucideIcons.Headphones, ToolHues.read) { vm.startReadAloud(doc) },
                                QuickChip(stringResource(R.string.common_tool_create_pdf), QuickTool.Create.icon, ToolHues.create) { vm.runQuickTool(doc, QuickTool.Create) },
                                QuickChip(stringResource(R.string.common_tool_organize), QuickTool.Organize.icon, ToolHues.organize) { vm.runQuickTool(doc, QuickTool.Organize) },
                            ),
                            onExtraHeight = { quickExtraPx = it },
                        )
                    } else {
                        ToolSubToolbar(vm, doc)
                    }
                }
            }
            // the navigation bar's band stays solid while the bars show (also while searching)
            if (ctl.barsVisible) Box(Modifier.fillMaxWidth().windowInsetsBottomHeight(WindowInsets.navigationBars).background(c.navBand))
        }

        // The active tool's sheets, over everything.
        ToolSheets(vm, doc)

        // Text selection's menu.
        selection.selection?.let { sel ->
            SelectionHandles(doc, sel)
            // out of the way while a handle is dragged; back where the selection ends
            if (selection.magnifierAt == null) SelectionMenu(vm, doc, sel)
        }

        // Sheets and dialogs.
        SheetHost(ctl.sheet.takeIf { it != ViewerSheet.None }) { sheet ->
            when (sheet) {
                ViewerSheet.More -> MoreSheet(vm, doc)
                ViewerSheet.ViewSettings -> ViewSettingsSheet(vm, doc)
                ViewerSheet.Bookmarks -> BookmarksSheet(vm, doc)
                ViewerSheet.Attachments -> AttachmentsSheet(vm, doc)
                ViewerSheet.Comments -> Unit
                ViewerSheet.MoreTools -> MoreToolsSheet(vm, doc)
                ViewerSheet.Signatures -> SignaturesSheet(vm, doc)
                ViewerSheet.None -> Unit
            }
        }
        if (ctl.sheet == ViewerSheet.Comments) CommentListScreen(vm, doc)
        vm.pageMenu?.takeIf { it.doc === doc }?.let { PageMenuView(vm, it) }
        if (vm.imageSource?.first === doc) {
            io.github.dikshant363.pdfcraft.ui.PdfSheet(onDismiss = { vm.imageSource = null }, dim = true) {
                io.github.dikshant363.pdfcraft.ui.SheetTitle(stringResource(R.string.viewer_add_image))
                io.github.dikshant363.pdfcraft.ui.SheetRow(LucideIcons.Images, stringResource(R.string.viewer_image_from_photos), { vm.addImageFrom(camera = false) })
                io.github.dikshant363.pdfcraft.ui.SheetRow(LucideIcons.Camera, stringResource(R.string.viewer_image_from_camera), { vm.addImageFrom(camera = true) })
                Spacer(Modifier.height(12.dp))
            }
        }
        vm.notePrompt?.takeIf { it.doc === doc }?.let { NoteSheet(vm, it) }
        vm.voicePrompt?.takeIf { it.doc === doc }?.let { VoiceSheet(vm, it) }
        if (vm.micRationale) MicRationaleDialog(vm)
        vm.selectedComment?.takeIf { it.first === doc }?.let { (_, cm) -> CommentPanel(vm, doc, cm) }
        // over the panel's tap-to-close area, but only where the comment is: drag it to move, corners to resize
        if (ctl.tool == ViewerTool.None && doc.info.allowsAnnotation) {
            vm.selectedComment?.takeIf { it.first === doc }?.second?.let { c ->
                CommentDragOverlay(doc, c, onMove = { dx, dy -> vm.moveComment(doc, c, dx, dy) }, onResize = { vm.resizeComment(doc, c, it) })
            }
        }
        vm.replyPrompt?.takeIf { it.doc === doc }?.let { ReplySheet(vm, it) }
        vm.readAloud?.let { ReadAloudBar(vm, doc, it) }
        if (ctl.goToPage) GoToPageDialog(doc, onDismiss = { ctl.goToPage = false })
        if (ctl.pagesOpen) PagesScreen(vm, doc, onClose = { ctl.pagesOpen = false })
    }
}

/** -1 for a tap in the left edge strip, 1 in the right one, else 0 (18 % of the width, at most 150 px). */
fun edgeTap(x: Float, width: Float): Int {
    val strip = minOf(width * 0.18f, 150f)
    return when {
        x < strip -> -1
        x > width - strip -> 1
        else -> 0
    }
}

private fun toolHint(tool: ViewerTool, markup: TextMarkup, doc: OpenDocument): String? = when (tool) {
    ViewerTool.Markup -> when (markup) {
        TextMarkup.Highlight -> text(R.string.viewer_hint_highlight)
        TextMarkup.Underline -> text(R.string.viewer_hint_underline)
        TextMarkup.Strikethrough -> text(R.string.viewer_hint_strikethrough)
    }
    ViewerTool.Draw -> text(R.string.viewer_hint_draw)
    ViewerTool.Comment -> text(R.string.viewer_hint_comment)
    ViewerTool.Text -> text(R.string.viewer_hint_text)
    ViewerTool.FillSign -> if (doc.info.fields > 0u) null else text(R.string.viewer_hint_fill_sign)
    else -> null
}

private fun runSearch(vm: AppViewModel, doc: OpenDocument, query: String) {
    if (query.isEmpty()) return
    doc.search.run(query, doc.pageCount, doc.viewer.currentPage) { first -> showHit(doc, first) }
    vm.whenSearchDone(doc)
}

private fun closeSearch(doc: OpenDocument) {
    doc.search.clear()
    doc.controller.searching = false
}

/** Bring a hit into view: its page, with the hit a third of the way down the screen. */
fun showHit(doc: OpenDocument, hit: io.github.dikshant363.pdfcraft.core.SearchHit) {
    val page = hit.range.page.toInt()
    val y = hit.rects.getOrNull(1) ?: 0f
    val v = doc.viewer
    if (v.mode == PageMode.SinglePage) {
        if (v.currentPage != page) v.goToPage(page)
        return
    }
    val screen = v.pageToScreen(page, hit.rects.getOrNull(0) ?: 0f, y)
    if (screen.y < v.padTop || screen.y > v.viewH - v.padBottom - 40f || v.currentPage != page && (screen.y < 0 || screen.y > v.viewH)) {
        v.setScroll(v.scrollX + (screen.x - v.viewW / 2f).coerceAtLeast(-v.scrollX).let { if (v.zoom <= 1f) 0f else it }, v.scrollY + screen.y - v.viewH / 3f)
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawSearchHits(doc: OpenDocument) {
    val s = doc.search
    if (s.hits.isEmpty()) return
    val visible = doc.viewer.visiblePages()
    val current = s.currentHit
    for (hit in s.hits) {
        val page = hit.range.page.toInt()
        if (page !in visible) continue
        val isCurrent = hit === current
        hit.rects.toList().chunked(4).forEach { r ->
            if (r.size < 4) return@forEach
            val a = doc.viewer.pageToScreen(page, r[0], r[1])
            val b = doc.viewer.pageToScreen(page, r[2], r[3])
            drawRect(
                if (isCurrent) Color(0x99E8461E) else Color(0x66FFE24D),
                topLeft = Offset(a.x - 1f, a.y - 1f),
                size = Size(b.x - a.x + 2f, b.y - a.y + 2f),
            )
        }
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawSelection(doc: OpenDocument) {
    val sel = doc.selection.selection ?: return
    val page = sel.range.page.toInt()
    for (r in sel.rects) {
        val a = doc.viewer.pageToScreen(page, r[0], r[1])
        val b = doc.viewer.pageToScreen(page, r[2], r[3])
        // the reference's light cyan, multiplied so the text stays black
        drawRect(Color(0xFFADE1F5), topLeft = a, size = Size(b.x - a.x, b.y - a.y), blendMode = androidx.compose.ui.graphics.BlendMode.Multiply)
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawCommentSelection(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo) {
    if (c.rect.size < 4) return
    val page = c.page.toInt()
    val a = doc.viewer.pageToScreen(page, c.rect[0], c.rect[1])
    val b = doc.viewer.pageToScreen(page, c.rect[2], c.rect[3])
    val dp = density
    when (c.subtype) {
        // marked text shows no frame
        "Highlight", "Underline", "StrikeOut", "Squiggly" -> Unit
        // a note's icon gets a ring: a white gap, then blue, then a thin white edge
        "Text" -> {
            // around the app's bubble, following its shape
            val cx = (a.x + b.x) / 2f
            val cy = (a.y + b.y) / 2f
            val w = NOTE_SELECTED_DP * dp
            drawPath(noteBubble(cx, cy, w + 2 * 1.4f * dp), Color.White, style = androidx.compose.ui.graphics.drawscope.Stroke(1.4f * dp))
            drawPath(noteBubble(cx, cy, w + 2 * 2.8f * dp), Color(0xFF2680EB), style = androidx.compose.ui.graphics.drawscope.Stroke(1.4f * dp))
            drawPath(noteBubble(cx, cy, w + 2 * 3.85f * dp), Color.White, style = androidx.compose.ui.graphics.drawscope.Stroke(0.7f * dp))
        }
        "Sound" -> {
            val center = Offset((a.x + b.x) / 2f, (a.y + b.y) / 2f)
            val r = maxOf(b.x - a.x, b.y - a.y) / 2f + 1.4f * dp
            drawCircle(Color.White, r + 1.4f * dp + 0.35f * dp, center, style = androidx.compose.ui.graphics.drawscope.Stroke(0.7f * dp))
            drawCircle(Color(0xFF2680EB), r + 0.7f * dp, center, style = androidx.compose.ui.graphics.drawscope.Stroke(1.4f * dp))
            drawCircle(Color.White, r - 0.7f * dp, center, style = androidx.compose.ui.graphics.drawscope.Stroke(1.4f * dp))
        }
        else -> drawRect(
            Color(0xFF448AFF), topLeft = Offset(a.x - 2f * dp, a.y - 2f * dp), size = Size(b.x - a.x + 4f * dp, b.y - a.y + 4f * dp),
            style = androidx.compose.ui.graphics.drawscope.Stroke(2f * dp),
        )
    }
}

/** A sticky note's icon on screen, whatever the zoom (dp); a selected one is drawn a little smaller inside its ring. */
private const val NOTE_ICON_DP = 21f
private const val NOTE_SELECTED_DP = 19f

/**
 * Sticky notes, drawn by the app at a fixed size as the reference shows them (the renderer leaves
 * them out): a speech bubble, three corners round and the lower right one tight. At rest it is a
 * pale fill in a 2 dp outline of the note's colour, edged in white over a soft shadow; selected,
 * it fills with the colour and shows two white lines of "text".
 */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawNotes(doc: OpenDocument, night: Boolean, selected: io.github.dikshant363.pdfcraft.core.CommentInfo?) {
    val dp = density
    val s = NOTE_ICON_DP * dp
    for (n in doc.comments) {
        if (n.subtype != "Text" || n.replyTo != null || n.state != null || n.fillSign || n.rect.size < 4) continue
        val page = n.page.toInt()
        val a = doc.viewer.pageToScreen(page, n.rect[0], n.rect[1])
        val b = doc.viewer.pageToScreen(page, n.rect[2], n.rect[3])
        val cx = (a.x + b.x) / 2f
        val cy = (a.y + b.y) / 2f
        if (cx < -s || cy < -s || cx > size.width + s || cy > size.height + s) continue
        val isSelected = selected != null && selected.page == n.page && selected.index == n.index
        fun bubble(inset: Float) = noteBubble(cx, cy, s - 2 * inset)
        val colour = if (night) Color(0xFF212121) else n.color?.let { Color(0xFF000000 or it.toLong()) } ?: Color(0xFFFAD900)
        if (isSelected) {
            val sel = noteBubble(cx, cy, NOTE_SELECTED_DP * dp)
            drawPath(sel, colour)
            for (f in listOf(0.40f, 0.60f)) {
                val y = cy - s / 2f + s * f
                drawLine(Color.White, Offset(cx - s * 0.24f, y), Offset(cx + s * (if (f < 0.5f) 0.22f else 0.02f), y), strokeWidth = 1.5f * dp, cap = androidx.compose.ui.graphics.StrokeCap.Round)
            }
        } else {
            // a soft shadow, the white edge, then the outlined pale bubble
            translate(0.6f * dp, 1.2f * dp) { drawPath(bubble(-0.5f * dp), Color(0x33000000)) }
            drawPath(bubble(-1.5f * dp), Color.White)
            val pale = Color(colour.red * 0.45f + 0.55f, colour.green * 0.45f + 0.55f, colour.blue * 0.45f + 0.55f)
            drawPath(bubble(0f), pale)
            drawPath(bubble(1f * dp), colour, style = androidx.compose.ui.graphics.drawscope.Stroke(2f * dp))
        }
    }
}

/** The note bubble [w] wide centred on ([cx], [cy]): three corners round, the lower right tight. */
private fun noteBubble(cx: Float, cy: Float, w: Float): androidx.compose.ui.graphics.Path {
    val l = cx - w / 2f
    val t = cy - w / 2f
    val round = androidx.compose.ui.geometry.CornerRadius(w / 2f)
    val tight = androidx.compose.ui.geometry.CornerRadius(w * 0.21f)
    return androidx.compose.ui.graphics.Path().apply { addRoundRect(androidx.compose.ui.geometry.RoundRect(l, t, l + w, t + w, round, round, tight, round)) }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawLinkFlash(doc: OpenDocument) {
    val f = doc.linkFlash ?: return
    val a = doc.viewer.pageToScreen(f.page, f.rect[0], f.rect[1])
    val b = doc.viewer.pageToScreen(f.page, f.rect[2], f.rect[3])
    drawRect(Color(0x66B36B2E), topLeft = a, size = Size(b.x - a.x, b.y - a.y))
}

/** A link's area flashing after a tap. */
data class LinkFlash(val page: Int, val rect: FloatArray)

/** The green banner a finished job leaves over the bar: a tick, the message, close, and Undo. */
@Composable
private fun SuccessBannerView(b: io.github.dikshant363.pdfcraft.ui.SuccessBanner, onClose: () -> Unit) {
    LaunchedEffect(b) {
        delay(8000)
        onClose()
    }
    Column(Modifier.fillMaxWidth().background(Color(0xFF12805C)).padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 4.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Icon(LucideIcons.CircleCheck, null, tint = Color.White, modifier = Modifier.padding(top = 2.dp).size(22.dp))
            Spacer(Modifier.width(14.dp))
            Text(b.text, color = Color.White, fontSize = 17.sp, lineHeight = 22.sp, modifier = Modifier.weight(1f))
            BarIcon(LucideIcons.X, stringResource(R.string.viewer_close), onClose, tint = Color.White, size = 40.dp)
        }
        b.undo?.let { undo ->
            Text(
                stringResource(R.string.common_undo), color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.align(Alignment.End).clickable { onClose(); undo() }.padding(horizontal = 24.dp, vertical = 12.dp),
            )
        }
    }
}
