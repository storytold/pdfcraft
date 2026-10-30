package com.storytold.pdfcraft.kit

import android.os.Environment
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.SaveAlt
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.pdfcraft_ffi.*
import java.io.File

/**
 * Sanitizes tool strings to remove any product references and format markdown placeholders.
 */
fun sanitizeUserText(text: String): String {
    if (text.isBlank()) return text
    return text
        .replace(Regex("(?i)\\b(acrobat|adobe)('s)?\\b"), "")
        .replace("`path`", "the destination")
        .replace("`written`", "status")
        .replace(Regex("\\s{2,}"), " ")
        .trim()
}

/**
 * Detects if a schema parameter represents a file system destination.
 */
private fun isFilePathField(name: String, desc: String): Boolean {
    val n = name.lowercase()
    val d = desc.lowercase()
    return n == "path" || n == "target" || n == "destination" || n == "output" ||
        n.endsWith("path") || d.contains("file path") || d.contains("write a") ||
        d.contains("destination") || d.contains("target path")
}

/**
 * Dynamic JSON Schema-driven form renderer for automation tools (Phase 5 / MOB-5.2).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfToolFormView(
    tool: FfiToolInfo,
    document: PdfDocument?,
    engine: Engine = Engine(),
    onComplete: ((FfiToolResult) -> Unit)? = null,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val coroutineScope = rememberCoroutineScope()
    val scrollState = rememberScrollState()

    val fields = remember(tool) {
        try {
            uniffi.pdfcraft_ffi.parseToolSchema(tool.inputSchemaJson)
        } catch (_: Throwable) {
            emptyList<FfiSchemaField>()
        }
    }

    val defaultFileName = remember(tool, document) {
        val baseName = document?.name?.substringBeforeLast('.') ?: "document"
        when (tool.name) {
            "doc_reduce" -> "${baseName}_reduced.pdf"
            "doc_optimize" -> "${baseName}_optimized.pdf"
            "doc_flatten" -> "${baseName}_flattened.pdf"
            "doc_export_data" -> "${baseName}_data.xml"
            "export_text" -> "${baseName}.txt"
            "sign_document" -> "${baseName}_signed.pdf"
            "print" -> "${baseName}_print.pdf"
            else -> "${baseName}_${tool.name.removePrefix("doc_")}.pdf"
        }
    }

    val docsDir = remember(context) {
        context.getExternalFilesDir(Environment.DIRECTORY_DOCUMENTS) ?: context.filesDir
    }

    val stringValues = remember { mutableStateMapOf<String, String>() }
    val boolValues = remember { mutableStateMapOf<String, Boolean>() }
    var isRunning by remember { mutableStateOf(false) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    var resultText by remember { mutableStateOf<String?>(null) }
    var resultStatusMessage by remember { mutableStateOf<String?>(null) }
    var showConfirmation by remember { mutableStateOf(false) }

    // State for renaming output file
    var showRenameDialog by remember { mutableStateOf(false) }
    var editingPathField by remember { mutableStateOf<String?>(null) }
    var editingFileName by remember { mutableStateOf("") }

    LaunchedEffect(fields) {
        for (f in fields) {
            if (f.fieldType == FfiSchemaFieldType.BOOLEAN) {
                boolValues[f.name] = f.defaultValue?.toBooleanStrictOrNull() ?: false
            } else if (isFilePathField(f.name, f.description)) {
                if (stringValues[f.name].isNullOrBlank()) {
                    stringValues[f.name] = f.defaultValue?.ifBlank { null }
                        ?: File(docsDir, defaultFileName).absolutePath
                }
            } else {
                stringValues[f.name] = f.defaultValue ?: ""
            }
        }
    }

    fun buildJsonArgs(): String {
        val pairs = mutableListOf<String>()
        if (document != null) {
            pairs.add("\"doc\": ${document.docId}")
        }
        for (f in fields) {
            if (f.name == "doc") continue
            if (f.fieldType == FfiSchemaFieldType.BOOLEAN) {
                pairs.add("\"${f.name}\": ${boolValues[f.name] ?: false}")
            } else if (f.fieldType == FfiSchemaFieldType.INTEGER) {
                val v = stringValues[f.name]?.trim()?.toLongOrNull() ?: 0L
                pairs.add("\"${f.name}\": $v")
            } else if (f.fieldType == FfiSchemaFieldType.NUMBER) {
                val v = stringValues[f.name]?.trim()?.toDoubleOrNull() ?: 0.0
                pairs.add("\"${f.name}\": $v")
            } else {
                val s = stringValues[f.name]?.replace("\"", "\\\"") ?: ""
                pairs.add("\"${f.name}\": \"$s\"")
            }
        }
        return "{${pairs.joinToString(", ")}}"
    }

    fun executeTool() {
        isRunning = true
        errorMessage = null
        resultText = null
        resultStatusMessage = null
        coroutineScope.launch {
            try {
                val json = buildJsonArgs()
                val res = withContext(Dispatchers.Default) {
                    engine.invokeTool(tool.name, json)
                }
                isRunning = false
                if (res.success) {
                    resultText = res.outputJson
                    try {
                        val parsed = JSONObject(res.outputJson)
                        if (parsed.has("written")) {
                            val written = parsed.optBoolean("written", true)
                            if (written) {
                                val savedFile = stringValues["path"]?.let { File(it).name } ?: defaultFileName
                                resultStatusMessage = "Saved copy to $savedFile"
                            } else {
                                resultStatusMessage = "Document is already optimally compressed. No copy needed."
                            }
                        } else {
                            val savedFile = stringValues["path"]?.let { File(it).name }
                            if (savedFile != null) {
                                resultStatusMessage = "Saved output to $savedFile"
                            }
                        }
                    } catch (_: Throwable) {
                    }
                    onComplete?.invoke(res)
                } else {
                    errorMessage = res.error ?: "Operation failed"
                }
            } catch (e: Throwable) {
                isRunning = false
                errorMessage = e.message ?: "Execution error"
            }
        }
    }

    Surface(
        shape = RoundedCornerShape(16.dp),
        tonalElevation = 6.dp,
        modifier = modifier.fillMaxWidth()
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(20.dp)
                .verticalScroll(scrollState)
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Text(
                    text = sanitizeUserText(tool.title),
                    style = MaterialTheme.typography.titleLarge,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Close")
                }
            }

            Text(
                text = sanitizeUserText(tool.description),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )

            if (tool.destructive) {
                Spacer(modifier = Modifier.height(12.dp))
                Surface(
                    shape = RoundedCornerShape(8.dp),
                    color = MaterialTheme.colorScheme.errorContainer.copy(alpha = 0.5f)
                ) {
                    Text(
                        text = "Warning: This operation modifies or removes document content.",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.padding(10.dp)
                    )
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            // Dynamic parameter rows
            val formFields = fields.filter { it.name != "doc" }
            if (formFields.isNotEmpty()) {
                Text(
                    text = "Parameters",
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.SemiBold
                )
                Spacer(modifier = Modifier.height(8.dp))

                for (f in formFields) {
                    val isPath = isFilePathField(f.name, f.description)
                    Column(modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                        if (!isPath) {
                            Text(
                                text = "${sanitizeUserText(f.title)}${if (f.required) " *" else ""}",
                                style = MaterialTheme.typography.labelMedium
                            )
                            if (f.description.isNotBlank()) {
                                Text(
                                    text = sanitizeUserText(f.description),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.secondary
                                )
                            }
                        }

                        if (f.fieldType == FfiSchemaFieldType.BOOLEAN) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.SpaceBetween,
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Text(text = if (boolValues[f.name] == true) "Enabled" else "Disabled")
                                Switch(
                                    checked = boolValues[f.name] ?: false,
                                    onCheckedChange = { boolValues[f.name] = it }
                                )
                            }
                        } else if (isPath) {
                            val currentPath = stringValues[f.name] ?: File(docsDir, defaultFileName).absolutePath
                            val currentFileName = File(currentPath).name.ifBlank { defaultFileName }

                            Card(
                                modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                                shape = RoundedCornerShape(12.dp),
                                colors = CardDefaults.cardColors(
                                    containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f)
                                )
                            ) {
                                Column(modifier = Modifier.padding(14.dp)) {
                                    Row(
                                        verticalAlignment = Alignment.CenterVertically,
                                        horizontalArrangement = Arrangement.SpaceBetween,
                                        modifier = Modifier.fillMaxWidth()
                                    ) {
                                        Row(
                                            verticalAlignment = Alignment.CenterVertically,
                                            modifier = Modifier.weight(1f)
                                        ) {
                                            Icon(
                                                Icons.Outlined.SaveAlt,
                                                contentDescription = null,
                                                tint = MaterialTheme.colorScheme.primary,
                                                modifier = Modifier.size(24.dp)
                                            )
                                            Spacer(modifier = Modifier.width(10.dp))
                                            Column {
                                                Text(
                                                    text = "Save Destination",
                                                    style = MaterialTheme.typography.labelMedium,
                                                    color = MaterialTheme.colorScheme.primary,
                                                    fontWeight = FontWeight.SemiBold
                                                )
                                                Text(
                                                    text = currentFileName,
                                                    style = MaterialTheme.typography.bodyMedium,
                                                    fontWeight = FontWeight.Bold,
                                                    maxLines = 1,
                                                    overflow = TextOverflow.Ellipsis
                                                )
                                            }
                                        }
                                        TextButton(
                                            onClick = {
                                                editingPathField = f.name
                                                editingFileName = currentFileName
                                                showRenameDialog = true
                                            }
                                        ) {
                                            Text("Change")
                                        }
                                    }
                                    Spacer(modifier = Modifier.height(4.dp))
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Icon(
                                            Icons.Outlined.Folder,
                                            contentDescription = null,
                                            tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.7f),
                                            modifier = Modifier.size(14.dp)
                                        )
                                        Spacer(modifier = Modifier.width(4.dp))
                                        Text(
                                            text = "Saved in app Documents folder",
                                            style = MaterialTheme.typography.labelSmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.8f)
                                        )
                                    }
                                }
                            }
                        } else {
                            OutlinedTextField(
                                value = stringValues[f.name] ?: "",
                                onValueChange = { stringValues[f.name] = it },
                                singleLine = true,
                                modifier = Modifier.fillMaxWidth()
                            )
                        }
                    }
                }
            }

            errorMessage?.let { err ->
                Spacer(modifier = Modifier.height(12.dp))
                Text(
                    text = err,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall
                )
            }

            resultStatusMessage?.let { status ->
                Spacer(modifier = Modifier.height(12.dp))
                Surface(
                    shape = RoundedCornerShape(8.dp),
                    color = MaterialTheme.colorScheme.primaryContainer,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.padding(12.dp)
                    ) {
                        Icon(
                            Icons.Outlined.CheckCircle,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(20.dp)
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        Text(
                            text = status,
                            style = MaterialTheme.typography.bodyMedium,
                            fontWeight = FontWeight.Medium,
                            color = MaterialTheme.colorScheme.onPrimaryContainer
                        )
                    }
                }
            }

            resultText?.let { out ->
                Spacer(modifier = Modifier.height(8.dp))
                Surface(
                    shape = RoundedCornerShape(8.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.4f),
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Text(
                        text = out,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                        modifier = Modifier.padding(8.dp)
                    )
                }
            }

            Spacer(modifier = Modifier.height(20.dp))

            Button(
                onClick = {
                    if (tool.destructive) {
                        showConfirmation = true
                    } else {
                        executeTool()
                    }
                },
                enabled = !isRunning,
                colors = ButtonDefaults.buttonColors(
                    containerColor = if (tool.destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary
                ),
                modifier = Modifier.fillMaxWidth()
            ) {
                if (isRunning) {
                    CircularProgressIndicator(modifier = Modifier.size(20.dp), color = MaterialTheme.colorScheme.onPrimary)
                    Spacer(modifier = Modifier.width(8.dp))
                }
                Text(if (tool.destructive) "Confirm & Run" else "Run ${sanitizeUserText(tool.title)}")
            }
        }
    }

    if (showRenameDialog && editingPathField != null) {
        AlertDialog(
            onDismissRequest = { showRenameDialog = false },
            title = { Text("Rename output file") },
            text = {
                Column {
                    Text("Enter the name for the output file:", style = MaterialTheme.typography.bodyMedium)
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = editingFileName,
                        onValueChange = { editingFileName = it },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        val fieldName = editingPathField
                        if (fieldName != null && editingFileName.isNotBlank()) {
                            val cleanName = if (editingFileName.contains('.')) editingFileName else "$editingFileName.pdf"
                            stringValues[fieldName] = File(docsDir, cleanName).absolutePath
                        }
                        showRenameDialog = false
                    }
                ) {
                    Text("Set")
                }
            },
            dismissButton = {
                TextButton(onClick = { showRenameDialog = false }) {
                    Text("Cancel")
                }
            }
        )
    }

    if (showConfirmation) {
        AlertDialog(
            onDismissRequest = { showConfirmation = false },
            title = { Text("Confirm Action") },
            text = { Text("Are you sure you want to run '${sanitizeUserText(tool.title)}'? This operation will modify the document.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        showConfirmation = false
                        executeTool()
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)
                ) {
                    Text("Execute")
                }
            },
            dismissButton = {
                TextButton(onClick = { showConfirmation = false }) {
                    Text("Cancel")
                }
            }
        )
    }
}
