package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.annotation.StringRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.engine.DigitalId
import io.github.dikshant363.pdfcraft.text as appText
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfDialog
import io.github.dikshant363.pdfcraft.ui.PdfField
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.SheetDivider
import io.github.dikshant363.pdfcraft.ui.SheetRow
import io.github.dikshant363.pdfcraft.ui.SheetTitle
import io.github.dikshant363.pdfcraft.ui.pdfColors
import io.github.dikshant363.pdfcraft.ui.safeTopPadding
import java.text.DateFormat
import java.util.Date
import org.json.JSONObject

// Digital signatures: signing with a digital ID (a certificate in a .p12 / .pfx file, or one made
// here), and what the signatures in a document say. The engine signs (PAdES, incrementally) and
// validates; these are the screens around it.

/** A signer's certificate, as a signature's details show it. */
data class SignerCert(val name: String, val issuer: String, val validTo: String?, val sha256: String, val selfSigned: Boolean)

/** One signature field and what validating it found (the engine's `sign_list`). */
data class SignatureEntry(
    val field: String,
    val signed: Boolean,
    /** `valid`, `unknown` (intact, but the signer isn't trusted), `invalid` or `unsigned`. */
    val status: String,
    val signer: String?,
    val date: String?,
    val reason: String?,
    val location: String?,
    /** 0-based, for a visible signature or field. */
    val page: Int?,
    /** Points from the page's top-left, for a visible one. */
    val rect: FloatArray?,
    /** Certification level (1 no changes, 2 form filling, 3 comments too), for a certifying signature. */
    val certify: Int?,
    /** Changes since signing: `none`, `allowed` or `disallowed`, and what they were. */
    val modification: String,
    val changes: List<String>,
    /** The engine's explanation, sentence by sentence. */
    val details: List<String>,
    val cert: SignerCert?,
)

private fun JSONObject.text(key: String): String? = if (!has(key) || isNull(key)) null else optString(key).trim().takeIf { it.isNotEmpty() }

private fun JSONObject.strings(key: String): List<String> =
    optJSONArray(key)?.let { a -> (0 until a.length()).mapNotNull { i -> a.optString(i).takeIf { it.isNotBlank() } } }.orEmpty()

/** The signatures `sign_list` returned. */
fun parseSignatures(o: JSONObject): List<SignatureEntry> {
    val list = o.optJSONArray("signatures") ?: return emptyList()
    return (0 until list.length()).mapNotNull { i ->
        val s = list.optJSONObject(i) ?: return@mapNotNull null
        val cert = s.optJSONObject("certificate")?.let { c ->
            SignerCert(c.text("name") ?: c.text("subject") ?: "", c.text("issuer") ?: "", c.text("valid_to"), c.text("sha256") ?: "", c.optBoolean("self_signed"))
        }
        val rect = s.optJSONArray("rect")?.takeIf { it.length() == 4 }?.let { r -> FloatArray(4) { r.optDouble(it).toFloat() } }
        SignatureEntry(
            field = s.optString("field"),
            signed = s.optBoolean("signed"),
            status = s.text("status") ?: "unsigned",
            signer = s.text("signer") ?: cert?.name?.takeIf { it.isNotEmpty() },
            date = s.text("date"),
            reason = s.text("reason"),
            location = s.text("location"),
            page = if (s.isNull("page")) null else s.optInt("page", 0).takeIf { it >= 1 }?.minus(1),
            rect = rect,
            certify = if (s.isNull("certify")) null else s.optInt("certify", 0).takeIf { it in 1..3 },
            modification = s.text("modification") ?: "none",
            changes = s.strings("changes"),
            details = s.strings("details"),
            cert = cert,
        )
    }
}

/** Where a digital signature goes: an empty signature field, or a box drawn on page [page]. */
class CertTarget(val doc: OpenDocument, val field: String?, val page: Int, val rect: FloatArray?)

/** What a digital signature shows on the page beside the signer's details. */
enum class CertLook(@StringRes val label: Int) { Name(R.string.sign_look_name), Typed(R.string.sign_look_typed), Drawn(R.string.sign_look_drawn) }

/** A digital ID file picked to add, waiting for its password ([wrong]: the last one didn't open it). */
class IdImport(val file: java.io.File, val name: String, val wrong: Boolean)

/** A signature's status in a few words, for the list and the message on opening. */
fun SignatureEntry.statusLabel(): String = when {
    !signed -> appText(R.string.sign_status_unsigned)
    status == "valid" -> appText(R.string.sign_status_valid)
    status == "invalid" -> appText(R.string.sign_status_invalid)
    else -> appText(R.string.sign_status_unknown)
}

private val ValidGreen = Color(0xFF12805C)
private val UnknownAmber = Color(0xFFE68619)

@Composable
private fun statusIcon(s: SignatureEntry): Pair<androidx.compose.ui.graphics.vector.ImageVector, Color> = when {
    !s.signed -> LucideIcons.CircleDashed to pdfColors.icon
    s.status == "valid" -> LucideIcons.ShieldCheck to ValidGreen
    s.status == "invalid" -> LucideIcons.CircleX to pdfColors.red
    else -> LucideIcons.TriangleAlert to UnknownAmber
}

/**
 * Drawing where a digital signature goes (Fill & Sign ▸ Sign ▸ Sign with a digital ID): drag a
 * box with one finger (two still scroll and zoom), or tap for a box of the usual size there.
 */
@Composable
fun CertPlaceLayer(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    var start by remember { mutableStateOf<Offset?>(null) }
    var end by remember { mutableStateOf<Offset?>(null) }
    Box(
        Modifier.fillMaxSize().toolGestures(
            doc,
            onDown = { start = it; end = it },
            onMove = { end = it },
            onUp = { p, moved ->
                val s = start
                start = null
                end = null
                // the hint's Cancel was tapped (it sits over this layer): nothing to place
                if (vm.certPlacing !== doc) return@toolGestures
                val at = s?.let { doc.viewer.screenToPage(it) } ?: return@toolGestures
                val page = doc.info.pages.getOrNull(at.page) ?: return@toolGestures
                val pw = page.width
                val ph = page.height
                val rect = if (moved) {
                    // the far corner in the same page's points, kept on that page
                    val ppp = doc.viewer.pixelsPerPoint(at.page).coerceAtLeast(0.01f)
                    val x1 = (at.x + (p.x - s.x) / ppp).coerceIn(0f, pw)
                    val y1 = (at.y + (p.y - s.y) / ppp).coerceIn(0f, ph)
                    floatArrayOf(minOf(at.x, x1), minOf(at.y, y1), maxOf(at.x, x1), maxOf(at.y, y1))
                } else {
                    null
                }
                vm.placeCert(doc, at.page, fitSignatureBox(rect, at.x, at.y, pw, ph))
            },
            onCancel = { start = null; end = null },
        ),
    ) {
        val s = start
        val e = end
        if (s != null && e != null) {
            Canvas(Modifier.fillMaxSize()) {
                val tl = Offset(minOf(s.x, e.x), minOf(s.y, e.y))
                val size = Size(kotlin.math.abs(e.x - s.x), kotlin.math.abs(e.y - s.y))
                drawRect(c.accent.copy(alpha = 0.12f), tl, size)
                drawRect(c.accent, tl, size, style = Stroke(2.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 4.dp.toPx()))))
            }
        }
        val top = with(LocalDensity.current) { doc.viewer.padTop.toDp() }
        Row(
            Modifier.align(Alignment.TopCenter).padding(top = top + 12.dp, start = 16.dp, end = 16.dp)
                .clip(RoundedCornerShape(8.dp)).background(c.surface).padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(stringResource(R.string.sign_place_hint), color = c.text, fontSize = 14.sp, modifier = Modifier.weight(1f, fill = false))
            TextButton(onClick = { vm.certPlacing = null }) { Text(stringResource(R.string.common_cancel), color = c.accent) }
        }
    }
}

/** The signature box: [rect] when it is big enough, else one of the usual size around ([x], [y]); kept on the page. */
internal fun fitSignatureBox(rect: FloatArray?, x: Float, y: Float, pageW: Float, pageH: Float): FloatArray {
    if (rect != null && rect[2] - rect[0] >= 40f && rect[3] - rect[1] >= 16f) return rect
    val w = minOf(180f, pageW)
    val h = minOf(54f, pageH)
    val cx = rect?.let { (it[0] + it[2]) / 2f } ?: x
    val cy = rect?.let { (it[1] + it[3]) / 2f } ?: y
    val left = (cx - w / 2f).coerceIn(0f, pageW - w)
    val top = (cy - h / 2f).coerceIn(0f, pageH - h)
    return floatArrayOf(left, top, left + w, top + h)
}

/**
 * Sign with a digital ID: choose the ID (or add or create one), how the signature looks, an
 * optional reason and place, then the ID's password signs and saves the document.
 */
@Composable
fun CertSignScreen(vm: AppViewModel, target: CertTarget) {
    val c = pdfColors
    androidx.activity.compose.BackHandler { vm.certSign = null }
    val ids = vm.digitalIds.all
    val chosen = vm.chosenId?.takeIf { it in ids }
    val hasDrawn = remember(vm.signaturesChanged) { vm.signatures.signature.isFile }
    var look by remember { mutableStateOf(CertLook.Name) }
    var reason by remember { mutableStateOf("") }
    var location by remember { mutableStateOf("") }
    var password by remember(chosen) { mutableStateOf("") }
    val unusable = chosen?.let { idProblem(it) }
    val canSign = chosen != null && unusable == null
    fun sign() {
        if (chosen != null && canSign) vm.signWithId(target, chosen, password, if (look == CertLook.Drawn && !hasDrawn) CertLook.Name else look, reason, location)
    }
    Column(Modifier.fillMaxSize().background(c.surface).pointerInput(Unit) { detectTapGestures() }.safeTopPadding().navigationBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { vm.certSign = null }) { Text(stringResource(R.string.common_cancel), color = c.accent) }
            Text(stringResource(R.string.common_tool_sign_digital_id), color = c.text, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = TextAlign.Center, maxLines = 1, overflow = TextOverflow.Ellipsis)
            TextButton(onClick = ::sign, enabled = canSign) { Text(stringResource(R.string.common_sign), color = if (canSign) c.accent else c.textSecondary, fontWeight = FontWeight.SemiBold) }
        }
        SheetDivider(inset = 0.dp)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            Section(stringResource(R.string.sign_section_id))
            if (ids.isEmpty()) {
                Text(
                    stringResource(R.string.sign_no_id),
                    color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
                )
            }
            for (id in ids) IdRow(vm, id, selected = id == chosen) { vm.chosenId = id }
            SheetRow(LucideIcons.FolderOpen, stringResource(R.string.sign_add_id_file), { vm.pickDigitalId() })
            SheetRow(LucideIcons.Plus, stringResource(R.string.sign_create_id), { vm.idCreate = true })
            SheetDivider(vertical = 6.dp)

            Section(stringResource(R.string.sign_section_appearance))
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (l in CertLook.entries) {
                    val enabled = l != CertLook.Drawn || hasDrawn
                    Chip(stringResource(l.label), look == l, enabled) { look = l }
                }
            }
            Text(
                when (look) {
                    CertLook.Name -> stringResource(R.string.sign_look_name_about)
                    CertLook.Typed -> stringResource(R.string.sign_look_typed_about)
                    CertLook.Drawn -> stringResource(if (hasDrawn) R.string.sign_look_drawn_about else R.string.sign_look_drawn_missing)
                },
                color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
            )
            SheetDivider(vertical = 6.dp)

            Section(stringResource(R.string.sign_section_reason))
            Box(Modifier.padding(horizontal = 16.dp)) { PdfField(reason, { reason = it }, stringResource(R.string.sign_reason_hint), keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next)) }
            Spacer(Modifier.height(12.dp))
            Section(stringResource(R.string.sign_section_location))
            Box(Modifier.padding(horizontal = 16.dp)) { PdfField(location, { location = it }, stringResource(R.string.sign_location_hint), keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next)) }
            Spacer(Modifier.height(12.dp))

            if (chosen != null) {
                Section(stringResource(R.string.sign_section_password, chosen.name))
                Box(Modifier.padding(horizontal = 16.dp)) {
                    PdfField(
                        password, { password = it; vm.certSignError = null }, stringResource(R.string.sign_id_password), password = true, error = vm.certSignError != null,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                        keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = { sign() }),
                    )
                }
            }
            (vm.certSignError ?: unusable)?.let { Text(it, color = c.red, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) }
            Text(
                stringResource(R.string.sign_saves_note),
                color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
            )
        }
    }
}

/**
 * A certificate's name as a person reads it: its common name (`CN=`), else its organisation
 * (`O=`), else the whole name ("CN=Example Root CA, O=Example" reads "Example Root CA").
 */
internal fun readableName(dn: String): String {
    fun part(key: String) = Regex("""(?:^|,\s*)$key=([^,]+)""").find(dn)?.groupValues?.get(1)?.trim()?.takeIf { it.isNotEmpty() }
    return part("CN") ?: part("O") ?: dn
}

/** Why [id] can't sign now, if it can't. */
private fun idProblem(id: DigitalId): String? = when {
    id.expired() -> appText(R.string.sign_id_expired, shortDate(id.validTo * 1000))
    id.notYetValid() -> appText(R.string.sign_id_not_yet_valid, shortDate(id.validFrom * 1000))
    else -> null
}

private fun shortDate(ms: Long): String = DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date(ms))

@Composable
private fun Section(title: String) {
    Text(title, color = pdfColors.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 10.dp, bottom = 6.dp))
}

@Composable
private fun Chip(label: String, selected: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    Text(
        label,
        color = when {
            !enabled -> c.textSecondary.copy(alpha = 0.5f)
            selected -> c.accent
            else -> c.text
        },
        fontSize = 14.sp,
        fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
        maxLines = 1,
        modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(if (selected) c.pressed else Color.Transparent)
            .clickable(enabled = enabled, onClick = onClick).padding(horizontal = 14.dp, vertical = 8.dp),
    )
}

/** A digital ID to choose: whose it is, who issued it and until when, and Remove. */
@Composable
private fun IdRow(vm: AppViewModel, id: DigitalId, selected: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    var confirm by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(onClick = onClick).padding(start = 16.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(if (selected) LucideIcons.CircleDot else LucideIcons.Circle, null, tint = if (selected) c.accent else c.icon, modifier = Modifier.size(24.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(id.name, color = c.text, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val by = if (id.selfSigned) stringResource(R.string.sign_id_self_signed) else stringResource(R.string.sign_id_issued_by, readableName(id.issuer))
            val until = idProblem(id) ?: stringResource(R.string.sign_id_valid_until, shortDate(id.validTo * 1000))
            Text("$by · $until", color = if (idProblem(id) != null) c.red else c.textSecondary, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        BarIcon(LucideIcons.Trash2, stringResource(R.string.sign_remove_id_description, id.name), { confirm = true }, size = 40.dp)
    }
    if (confirm) {
        PdfDialog(
            onDismiss = { confirm = false }, title = stringResource(R.string.sign_remove_id_title), confirm = stringResource(R.string.sign_remove), destructive = true,
            onConfirm = { confirm = false; vm.removeDigitalId(id) },
        ) { Text(stringResource(R.string.sign_remove_id_message, id.name), color = c.text, fontSize = 14.sp) }
    }
}

/** The password of a digital ID file being added. */
@Composable
fun IdPasswordDialog(vm: AppViewModel, imp: IdImport) {
    val c = pdfColors
    var pw by remember(imp) { mutableStateOf("") }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(imp) { runCatching { focus.requestFocus() } }
    PdfDialog(onDismiss = { vm.answerIdImport(null) }, title = stringResource(R.string.sign_id_password), confirm = stringResource(R.string.sign_add), onConfirm = { vm.answerIdImport(pw) }) {
        Text(stringResource(R.string.sign_id_password_prompt, imp.name), color = c.text, fontSize = 14.sp)
        Spacer(Modifier.height(14.dp))
        PdfField(
            pw, { pw = it }, stringResource(R.string.common_password), error = imp.wrong, password = true, focus = focus,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
            keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = { vm.answerIdImport(pw) }),
        )
        if (imp.wrong) Text(stringResource(R.string.common_wrong_password), color = c.red, fontSize = 13.sp, modifier = Modifier.padding(top = 8.dp))
    }
}

/** Create a digital ID: a self-signed certificate made on the phone, saved with a password. */
@Composable
fun CreateIdScreen(vm: AppViewModel) {
    val c = pdfColors
    androidx.activity.compose.BackHandler { vm.idCreate = false }
    var name by remember { mutableStateOf(vm.toolPrefs.author) }
    var email by remember { mutableStateOf("") }
    var organization by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var again by remember { mutableStateOf("") }
    val problem = when {
        name.isBlank() -> null
        password.isNotEmpty() && password.length < 8 -> stringResource(R.string.sign_password_short)
        again.isNotEmpty() && again != password -> stringResource(R.string.sign_password_mismatch)
        else -> null
    }
    val ok = name.isNotBlank() && password.length >= 8 && again == password
    Column(Modifier.fillMaxSize().background(c.surface).pointerInput(Unit) { detectTapGestures() }.safeTopPadding().navigationBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { vm.idCreate = false }) { Text(stringResource(R.string.common_cancel), color = c.accent) }
            Text(stringResource(R.string.sign_create_id), color = c.text, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = TextAlign.Center)
            TextButton(onClick = { vm.createDigitalId(name, email, organization, password) }, enabled = ok) {
                Text(stringResource(R.string.common_create), color = if (ok) c.accent else c.textSecondary, fontWeight = FontWeight.SemiBold)
            }
        }
        SheetDivider(inset = 0.dp)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            Text(
                stringResource(R.string.sign_create_about),
                color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
            )
            Section(stringResource(R.string.sign_section_name))
            Box(Modifier.padding(horizontal = 16.dp)) { PdfField(name, { name = it }, stringResource(R.string.common_your_name)) }
            Section(stringResource(R.string.sign_section_email))
            Box(Modifier.padding(horizontal = 16.dp)) { PdfField(email, { email = it }, stringResource(R.string.sign_email_hint), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email)) }
            Section(stringResource(R.string.sign_section_organisation))
            Box(Modifier.padding(horizontal = 16.dp)) { PdfField(organization, { organization = it }, stringResource(R.string.sign_organisation_hint)) }
            Section(stringResource(R.string.common_password))
            Box(Modifier.padding(horizontal = 16.dp)) {
                PdfField(password, { password = it }, stringResource(R.string.sign_password_hint), password = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password))
            }
            Spacer(Modifier.height(10.dp))
            Box(Modifier.padding(horizontal = 16.dp)) {
                PdfField(again, { again = it }, stringResource(R.string.sign_password_again), password = true, error = again.isNotEmpty() && again != password, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done))
            }
            problem?.let { Text(it, color = c.red, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) }
            Text(
                stringResource(R.string.sign_password_note),
                color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
            )
        }
    }
}

/** Tapping an empty signature field: draw a signature there, or sign it with a digital ID. */
@Composable
fun SignFieldChoiceDialog(vm: AppViewModel, doc: OpenDocument, field: io.github.dikshant363.pdfcraft.core.WidgetInfo) {
    PdfDialog(onDismiss = { vm.signFieldChoice = null }, title = null, confirm = null, dismiss = null, list = true) {
        SheetRow(LucideIcons.Signature, stringResource(R.string.sign_add_your_signature), { vm.signFieldChoice = null; vm.drawSignatureInField(doc, field) })
        SheetRow(LucideIcons.ShieldCheck, stringResource(R.string.common_tool_sign_digital_id), { vm.signFieldChoice = null; vm.signFieldWithId(doc, field) })
    }
}

/**
 * Signatures (More ▸ Signatures): each signature field with whether its signature is valid, who
 * signed and when, and on tap the details (certificate, changes since signing, the engine's
 * explanation), Trust this signer, and Sign for an empty field.
 */
@Composable
fun SignaturesSheet(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    fun close() {
        ctl.sheet = ViewerSheet.None
    }
    // read so Trust redraws the list
    vm.trustedCerts.version
    var open by remember { mutableStateOf<String?>(null) }
    PdfSheet(onDismiss = ::close, maxHeightFraction = 0.85f) {
        SheetTitle(stringResource(R.string.sign_signatures))
        Column(Modifier.verticalScroll(rememberScrollState())) {
            val list = doc.signatures
            if (list.isEmpty()) {
                Text(stringResource(R.string.sign_none), color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp))
            }
            for (s in list) {
                val (icon, tint) = statusIcon(s)
                Row(
                    Modifier.fillMaxWidth().clickable { open = if (open == s.field) null else s.field }.padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.Top,
                ) {
                    Icon(icon, null, tint = tint, modifier = Modifier.padding(top = 2.dp).size(24.dp))
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(if (s.signed) s.signer ?: stringResource(R.string.sign_unknown_signer) else stringResource(R.string.sign_empty_field), color = c.text, fontSize = 16.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        val sub = listOfNotNull(
                            s.statusLabel(),
                            if (s.certify != null && s.signed) stringResource(R.string.sign_certified) else null,
                            pdfDateLabel(s.date),
                            s.page?.let { stringResource(R.string.sheet_page, it + 1) }.takeIf { !s.signed },
                        ).joinToString(" · ")
                        Text(sub, color = if (s.status == "invalid") c.red else c.textSecondary, fontSize = 13.sp)
                        if (open == s.field) SignatureDetails(vm, doc, s)
                    }
                    Icon(if (open == s.field) LucideIcons.ChevronUp else LucideIcons.ChevronDown, null, tint = c.icon, modifier = Modifier.size(20.dp))
                }
                SheetDivider()
            }
            SheetRow(LucideIcons.ShieldCheck, stringResource(R.string.sign_add_signature), { close(); vm.startCertPlacing(doc) })
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun SignatureDetails(vm: AppViewModel, doc: OpenDocument, s: SignatureEntry) {
    val c = pdfColors
    @Composable
    fun line(text: String) {
        Text(text, color = c.text, fontSize = 14.sp, modifier = Modifier.padding(top = 6.dp))
    }
    /** The line [format] fills with [value], when there is a value. */
    @Composable
    fun detail(@StringRes format: Int, value: String?) {
        if (value.isNullOrBlank()) return
        line(stringResource(format, value))
    }
    Column(Modifier.padding(top = 4.dp)) {
        if (s.signed) {
            detail(R.string.sign_detail_reason, s.reason)
            detail(R.string.sign_detail_location, s.location)
            s.cert?.let { cert ->
                line(if (cert.selfSigned) stringResource(R.string.sign_detail_cert_self_signed) else stringResource(R.string.sign_detail_cert_issued_by, readableName(cert.issuer)))
                detail(R.string.sign_detail_valid_until, pdfDateLabel(cert.validTo))
            }
            s.certify?.let {
                line(
                    stringResource(
                        when (it) {
                            1 -> R.string.sign_detail_certified_no_changes
                            2 -> R.string.sign_detail_certified_forms
                            else -> R.string.sign_detail_certified_comments
                        },
                    ),
                )
            }
            // the changes are the engine's own words, listed as it gives them
            val changes = s.changes.joinToString("; ")
            line(
                when (s.modification) {
                    "none" -> stringResource(R.string.sign_detail_changes_none)
                    "allowed" -> if (s.changes.isEmpty()) stringResource(R.string.sign_detail_changes_allowed) else stringResource(R.string.sign_detail_changes_allowed_list, changes)
                    else -> if (s.changes.isEmpty()) stringResource(R.string.sign_detail_changes_disallowed) else stringResource(R.string.sign_detail_changes_disallowed_list, changes)
                },
            )
            for (d in s.details) Text("• $d", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp))
        } else {
            Text(stringResource(R.string.sign_field_unsigned), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp))
        }
        Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            if (s.page != null && s.rect != null) {
                TextButton(onClick = { doc.controller.sheet = ViewerSheet.None; vm.showSignature(doc, s) }) { Text(stringResource(R.string.sign_go_to), color = c.accent) }
            }
            val sha = s.cert?.sha256
            if (s.signed && sha != null) {
                if (vm.trustedCerts.contains(sha)) {
                    TextButton(onClick = { vm.untrustSigner(doc, s) }) { Text(stringResource(R.string.sign_stop_trusting), color = c.accent) }
                } else if (s.status == "unknown") {
                    TextButton(onClick = { vm.trustSigner(doc, s) }) { Text(stringResource(R.string.sign_trust), color = c.accent) }
                }
            }
            if (!s.signed) {
                TextButton(onClick = {
                    doc.controller.sheet = ViewerSheet.None
                    doc.widgets.firstOrNull { it.field == s.field }?.let { vm.signFieldWithId(doc, it) }
                }) { Text(stringResource(R.string.common_sign), color = c.accent) }
            }
        }
    }
}
