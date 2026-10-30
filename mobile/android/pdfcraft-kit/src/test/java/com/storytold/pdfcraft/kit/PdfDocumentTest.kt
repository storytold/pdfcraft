package com.storytold.pdfcraft.kit

import org.junit.Assert.*
import org.junit.BeforeClass
import org.junit.Test
import uniffi.pdfcraft_ffi.*
import java.io.File
import java.nio.charset.StandardCharsets

class PdfDocumentTest {
    companion object {
        private const val SAMPLE_PDF = "%PDF-1.7\n" +
            "1 0 obj\n" +
            "<< /Type /Catalog /Pages 2 0 R >>\n" +
            "endobj\n" +
            "2 0 obj\n" +
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n" +
            "endobj\n" +
            "3 0 obj\n" +
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\n" +
            "endobj\n" +
            "xref\n" +
            "0 4\n" +
            "0000000000 65535 f \n" +
            "0000000009 00000 n \n" +
            "0000000058 00000 n \n" +
            "0000000115 00000 n \n" +
            "trailer\n" +
            "<< /Size 4 /Root 1 0 R >>\n" +
            "startxref\n" +
            "193\n" +
            "%%EOF\n"

        private const val SAMPLE_PDF_WITH_TEXT = "%PDF-1.7\n" +
            "1 0 obj\n" +
            "<< /Type /Catalog /Pages 2 0 R >>\n" +
            "endobj\n" +
            "2 0 obj\n" +
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n" +
            "endobj\n" +
            "3 0 obj\n" +
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\n" +
            "endobj\n" +
            "4 0 obj\n" +
            "<< /Length 55 >>\n" +
            "stream\n" +
            "BT /F1 12 Tf 100 700 Td (CONFIDENTIAL SECRET AGENT) Tj ET\n" +
            "endstream\n" +
            "endobj\n" +
            "5 0 obj\n" +
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n" +
            "endobj\n" +
            "xref\n" +
            "0 6\n" +
            "0000000000 65535 f \n" +
            "0000000009 00000 n \n" +
            "0000000058 00000 n \n" +
            "0000000115 00000 n \n" +
            "0000000234 00000 n \n" +
            "0000000339 00000 n \n" +
            "trailer\n" +
            "<< /Size 6 /Root 1 0 R >>\n" +
            "startxref\n" +
            "424\n" +
            "%%EOF\n"

        @BeforeClass
        @JvmStatic
        fun setUpNativeLib() {
            val targetDir = File("../../../target/debug").canonicalFile
            if (targetDir.exists()) {
                System.setProperty("jna.library.path", targetDir.absolutePath)
            }
        }
    }

    @Test
    fun testPdfDocumentCreationAndPages() {
        val bytes = SAMPLE_PDF.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "sample.pdf")
        assertNotNull(doc)
        assertEquals(1, doc.pageCount)

        val geom = doc.geometry(0)
        assertNotNull(geom)
        assertEquals(612f, geom.width, 0.001f)
        assertEquals(792f, geom.height, 0.001f)

        // Insert blank page
        doc.insertBlankPage(afterPage = 0)
        assertEquals(2, doc.pageCount)

        // Rotate page
        doc.rotatePage(page = 0, degrees = 90)

        // Delete page
        doc.deletePage(page = 1)
        assertEquals(1, doc.pageCount)

        doc.close()
    }

    @Test
    fun testDocumentInfoAndMetadata() {
        val bytes = SAMPLE_PDF.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "info_test.pdf")

        assertEquals(1, doc.pageCount)
        val info = doc.info()
        assertNotNull(info)

        val bookmarks = doc.outline()
        assertEquals(0, bookmarks.size)

        val links = doc.links(0)
        assertEquals(0, links.size)

        val text = doc.text(0)
        assertTrue(text.isEmpty())

        doc.close()
    }

    @Test
    fun testToolCatalogueAndSchemaParsing() {
        val tools = listTools()
        assertTrue(tools.isNotEmpty())

        val bespoke = tools.filter { it.classification == FfiToolClassification.BESPOKE_SCREEN }
        val schemaDriven = tools.filter { it.classification == FfiToolClassification.SCHEMA_DRIVEN }
        val notOnMobile = tools.filter { it.classification == FfiToolClassification.NOT_ON_MOBILE }

        assertTrue(bespoke.isNotEmpty())
        assertTrue(schemaDriven.isNotEmpty())
        assertTrue(notOnMobile.isNotEmpty())

        for (tool in notOnMobile) {
            assertNotNull(tool.mobileReason)
            assertTrue(tool.mobileReason!!.isNotEmpty())
        }

        val firstSchemaTool = schemaDriven.first()
        val fields = parseToolSchema(firstSchemaTool.inputSchemaJson)
        assertNotNull(fields)
    }

    @Test
    fun testToolInvocationOnDocument() {
        val bytes = SAMPLE_PDF.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "tool_test.pdf")

        val geomBefore = doc.geometry(0)
        assertEquals(0u, geomBefore.rotation)

        val res = doc.invokeTool("page_rotate", """{"doc": ${doc.docId}, "pages": [1], "degrees": 90}""")
        assertTrue(res.success)
        assertTrue(res.outputJson.contains("\"rotated\":1") || res.outputJson.contains("\"rotated\": 1"))

        val geomAfter = doc.geometry(0)
        assertEquals(90u, geomAfter.rotation)

        val infoRes = doc.invokeTool("doc_info", """{"doc": ${doc.docId}}""")
        assertTrue(infoRes.success)
        assertTrue(infoRes.outputJson.contains("pdf_version"))

        doc.close()
    }

    @Test
    fun testRedactionMarkAuditAndApply() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "redact_test.pdf")

        val initialText = doc.text(0)
        assertTrue(initialText.contains("CONFIDENTIAL SECRET AGENT"))

        val initialFindings = doc.auditRedactions()
        val unapplied = initialFindings.filter { it.kind == "unapplied-mark" }
        assertEquals(0, unapplied.size)

        val markedCount = doc.markRedactionText("CONFIDENTIAL SECRET AGENT", "SECRET")
        assertTrue(markedCount >= 1u)

        val afterMarkFindings = doc.auditRedactions()
        assertTrue(afterMarkFindings.any { it.kind == "unapplied-mark" })

        val applyResult = doc.applyRedactions()
        assertTrue(applyResult.appliedCount >= 1u)
        assertTrue(applyResult.auditPassed)
        assertTrue(applyResult.auditFindings.isEmpty())

        val afterApplyFindings = doc.auditRedactions()
        val unappliedAfter = afterApplyFindings.filter { it.kind == "unapplied-mark" }
        assertEquals(0, unappliedAfter.size)

        val afterText = doc.text(0)
        assertFalse(afterText.contains("CONFIDENTIAL SECRET AGENT"))

        doc.close()
    }

    @Test
    fun testDigitalIdCreationInspectionAndDocumentSigning() {
        val p12Data = PdfDocument.createSelfSignedDigitalId(
            name = "Ada Lovelace",
            password = "supersecret123",
            keyType = "rsa2048",
            organization = "Storytold",
            unit = "Security",
            email = "ada@storytold.internal",
            country = "US",
            validityYears = 5u
        )
        assertTrue(p12Data.isNotEmpty())

        val idInfo = PdfDocument.inspectDigitalId(p12Bytes = p12Data, password = "supersecret123")
        assertEquals("Ada Lovelace", idInfo.commonName)
        assertEquals("Storytold", idInfo.organization)
        assertEquals("Security", idInfo.unit)
        assertEquals("ada@storytold.internal", idInfo.email)
        assertEquals("US", idInfo.country)
        assertTrue(idInfo.keyAlgorithm.contains("RSA 2048-bit"))
        assertTrue(idInfo.sha256Fingerprint.isNotEmpty())

        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "sign_test.pdf")

        val initialSigs = doc.listDigitalSignatures()
        assertEquals(0, initialSigs.size)

        val signOptions = FfiSignOptions(
            fieldName = null,
            page = 0u,
            rect = FfiRect(x = 50f, y = 50f, width = 220f, height = 60f),
            reason = "I certify and approve this document",
            location = "San Francisco, CA",
            contact = "ada@storytold.internal",
            certifyLevel = 1u,
            graphicText = "Ada Lovelace",
            graphicStrokes = null,
            outPath = null
        )

        val signResult = doc.sign(p12Bytes = p12Data, password = "supersecret123", options = signOptions)
        assertTrue(signResult.success)
        assertTrue(signResult.bytes.isNotEmpty())
        assertEquals("Ada Lovelace", signResult.signerName)
        assertEquals(1u, signResult.revision)

        val sigsAfter = doc.listDigitalSignatures()
        assertEquals(1, sigsAfter.size)
        val sig = sigsAfter[0]
        assertTrue(sig.signed)
        assertEquals("Ada Lovelace", sig.signer)
        assertEquals("I certify and approve this document", sig.reason)
        assertEquals("San Francisco, CA", sig.location)
        assertEquals(1.toUByte(), sig.certify)
        assertTrue(sig.status == "valid" || sig.status == "unknown")

        doc.close()
    }

    @Test
    fun testOcrStatusAndSearchableTextLayer() {
        val status = PdfDocument.ocrStatus()
        assertTrue(status.searchDirs.isNotEmpty())
        assertTrue(status.languages.any { it.contains("en: English") })

        val tempDir = System.getProperty("java.io.tmpdir") ?: "/tmp"
        PdfDocument.setOcrModelsDir(dir = tempDir)
        val updatedStatus = PdfDocument.ocrStatus()
        assertTrue(updatedStatus.searchDirs.contains(tempDir))

        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "ocr_test.pdf")

        val words = listOf(
            FfiOcrWord(text = "SEARCHABLE", rect = FfiRect(x = 50f, y = 150f, width = 90f, height = 18f)),
            FfiOcrWord(text = "KEYWORD", rect = FfiRect(x = 145f, y = 150f, width = 75f, height = 18f))
        )
        val applied = doc.applyOcrTextLayer(page = 0, words = words)
        assertEquals(2u, applied)

        val extracted = doc.text(0)
        assertTrue(extracted.contains("SEARCHABLE"))
        assertTrue(extracted.contains("KEYWORD"))

        val matches = doc.find(page = 0, query = "SEARCHABLE")
        assertEquals(1, matches.size)

        doc.close()
    }

    @Test
    fun testAdvancedCapabilitiesFormJsPreflightCompareAndA11y() {
        val engine = Engine()
        assertTrue(engine.isFormJavascriptEnabled())
        engine.setFormJavascriptEnabled(false)
        assertFalse(engine.isFormJavascriptEnabled())
        engine.setFormJavascriptEnabled(true)
        assertTrue(engine.isFormJavascriptEnabled())

        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(engine = engine, bytes = bytes, name = "advanced_test.pdf")
        assertTrue(doc.isFormJavascriptEnabled())
        doc.setFormJavascriptEnabled(false)
        assertFalse(doc.isFormJavascriptEnabled())
        doc.setFormJavascriptEnabled(true)
        assertTrue(doc.isFormJavascriptEnabled())

        val jsRes = doc.runJavascript("console.println('Kotlin test JS'); 1 + 1;")
        assertTrue(jsRes.success)
        assertTrue(jsRes.output.contains("Kotlin test JS"))

        val xfa = doc.xfaStatus
        assertEquals("none", xfa.kind)

        val issues = doc.verifyPdfA(level = "pdfa2b")
        assertTrue(issues.isNotEmpty())

        doc.convertToPdfA(level = "pdfa2b")
        val declared = doc.declaredStandards()
        assertEquals(2u, declared.pdfaPart)
        assertEquals("B", declared.pdfaConformance)

        val otherDoc = PdfDocument.openBytes(engine = engine, bytes = bytes, name = "other.pdf")
        val changes = doc.compare(otherDoc)
        assertNotNull(changes)

        val a11y = doc.checkAccessibility()
        assertEquals(32, a11y.results.size)
        assertEquals(32u, a11y.passedCount + a11y.failedCount + a11y.manualCheckCount + a11y.skippedCount)

        val fontStatus = doc.cjkFontStatus
        assertEquals(0u, fontStatus.craftFontsCount)
        assertFalse(fontStatus.hasJapaneseFont)
        assertFalse(fontStatus.hasChineseFont)

        otherDoc.close()
        doc.close()
    }

    @Test
    fun testConversionFeaturesExportTextImagesAndPdfA() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "conv_test.pdf")

        // 1. Export text to temporary file
        val tempTxt = File.createTempFile("export_test", ".txt")
        tempTxt.deleteOnExit()
        doc.exportText(tempTxt.absolutePath)
        val textContent = tempTxt.readText(StandardCharsets.UTF_8)
        assertTrue(textContent.contains("CONFIDENTIAL"))

        // 2. Export images to temporary directory
        val tempDir = java.nio.file.Files.createTempDirectory("export_images").toFile()
        doc.exportImages(tempDir.absolutePath, format = "png", dpi = 72f)
        val exportedImages = tempDir.listFiles { _, name -> name.endsWith(".png") }
        assertNotNull(exportedImages)
        assertTrue(exportedImages!!.isNotEmpty())

        // 3. Extract pages to a new PDF
        val tempExtracted = File.createTempFile("extracted", ".pdf")
        tempExtracted.deleteOnExit()
        doc.extractPages(listOf(0), tempExtracted.absolutePath)
        assertTrue(tempExtracted.exists() && tempExtracted.length() > 0)

        // 4. Convert to PDF/A
        val fixed = doc.convertToPdfA("pdfa2b")
        assertTrue(fixed >= 0u)

        tempDir.deleteRecursively()
        doc.close()
    }

    @Test
    fun testEditingFeaturesMetadataWatermarkHeaderFooterAndFlatten() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "edit_test.pdf")

        // 1. Set metadata
        doc.setMetadata(
            title = "Mobile Editing Test",
            author = "PdfCraft Test Suite",
            subject = "Automated Unit Test",
            keywords = "mobile, test, pdfcraft"
        )
        val info = doc.engine.docInfo(doc.docId)
        assertEquals("Mobile Editing Test", info.title)
        assertEquals("PdfCraft Test Suite", info.author)
        assertEquals("Automated Unit Test", info.subject)
        assertEquals("mobile, test, pdfcraft", info.keywords)

        // 2. Add Watermark
        doc.addWatermark(text = "TEST WATERMARK", opacity = 0.3f, rotation = 45.0)

        // 3. Add Header & Footer
        doc.addHeaderFooter(
            headerLeft = "Internal Draft",
            footerCenter = "Page 1 of 1"
        )

        // 4. Flatten document
        doc.flatten(comments = true, fields = true)

        doc.close()
    }

    @Test
    fun testSecurityPasswordProtectAndUnprotect() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "security_test.pdf")

        // Protect with open password
        doc.protect(
            openPassword = "UserPass123!",
            permissionsPassword = "OwnerPass456!",
            printing = "low-resolution",
            changes = "none",
            copy = false,
            accessibility = true
        )

        // Save bytes of protected document
        val protectedBytes = doc.saveBytes()
        assertTrue(protectedBytes.isNotEmpty())

        // Verify opening without password fails or opening with password succeeds
        val opened = PdfDocument.openBytes(bytes = protectedBytes, name = "opened_sec.pdf", password = "UserPass123!")
        assertEquals(1, opened.pageCount)
        opened.close()

        // Unprotect
        doc.unprotect()

        doc.close()
    }

    @Test
    fun testOutlineAndBookmarks() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "outline_test.pdf")

        val emptyOutline = doc.outline()
        assertNotNull(emptyOutline)
        assertTrue(emptyOutline.isEmpty())
        doc.close()

        val nestedOutlineFile = File("../../../corpus/pdfjs/test/pdfs/nested_outline.pdf")
        if (nestedOutlineFile.exists()) {
            val nestedDoc = PdfDocument.openFile(path = nestedOutlineFile.absolutePath)
            val outline = nestedDoc.outline()
            assertNotNull(outline)
            assertTrue(outline.isNotEmpty())
            val first = outline[0]
            assertNotNull(first.title)
            nestedDoc.close()
        }
    }

    @Test
    fun testInkDrawingStrokeEraseAndUndoRedo() {
        val bytes = SAMPLE_PDF_WITH_TEXT.toByteArray(StandardCharsets.UTF_8)
        val doc = PdfDocument.openBytes(bytes = bytes, name = "ink_test.pdf")

        // 1. Add ink stroke
        doc.addInkStroke(
            page = 0,
            points = listOf(
                uniffi.pdfcraft_ffi.FfiPoint(20f, 20f),
                uniffi.pdfcraft_ffi.FfiPoint(60f, 60f),
                uniffi.pdfcraft_ffi.FfiPoint(100f, 60f)
            ),
            colorRgb = 0x0000FFu,
            width = 3f
        )
        assertTrue(doc.isDirty)

        // 2. Undo ink
        val undone = doc.undo()
        assertNotNull(undone)

        // 3. Redo ink
        val redone = doc.redo()
        assertNotNull(redone)

        // 4. Erase ink
        doc.eraseInk(
            page = 0,
            path = listOf(uniffi.pdfcraft_ffi.FfiPoint(60f, 60f)),
            radius = 25f
        )

        doc.close()
    }
}
