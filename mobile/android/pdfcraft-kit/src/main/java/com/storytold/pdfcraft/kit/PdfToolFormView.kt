package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.pdfcraft_ffi.*

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
    val coroutineScope = rememberCoroutineScope()
    val scrollState = rememberScrollState()

    val fields = remember(tool) {
        try {
            uniffi.pdfcraft_ffi.parseToolSchema(tool.inputSchemaJson)
        } catch (_: Throwable) {
            emptyList<FfiSchemaField>()
        }
    }

    val stringValues = remember { mutableStateMapOf<String, String>() }
    val boolValues = remember { mutableStateMapOf<String, Boolean>() }
    var isRunning by remember { mutableStateOf(false) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    var resultText by remember { mutableStateOf<String?>(null) }
    var showConfirmation by remember { mutableStateOf(false) }

    LaunchedEffect(fields) {
        for (f in fields) {
            if (f.fieldType == FfiSchemaFieldType.BOOLEAN) {
                boolValues[f.name] = f.defaultValue?.toBooleanStrictOrNull() ?: false
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
        coroutineScope.launch {
            try {
                val json = buildJsonArgs()
                val res = withContext(Dispatchers.Default) {
                    engine.invokeTool(tool.name, json)
                }
                isRunning = false
                if (res.success) {
                    resultText = res.outputJson
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
                    text = tool.title,
                    style = MaterialTheme.typography.titleLarge,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Close")
                }
            }

            Text(
                text = tool.description,
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
                    Column(modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                        Text(
                            text = "${f.title}${if (f.required) " *" else ""}",
                            style = MaterialTheme.typography.labelMedium
                        )
                        if (f.description.isNotBlank()) {
                            Text(
                                text = f.description,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.secondary
                            )
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

            resultText?.let { out ->
                Spacer(modifier = Modifier.height(12.dp))
                Surface(
                    shape = RoundedCornerShape(8.dp),
                    color = MaterialTheme.colorScheme.primaryContainer.copy(alpha = 0.4f),
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
                Text(if (tool.destructive) "Confirm & Run" else "Run ${tool.title}")
            }
        }
    }

    if (showConfirmation) {
        AlertDialog(
            onDismissRequest = { showConfirmation = false },
            title = { Text("Confirm Destructive Action") },
            text = { Text("Are you sure you want to run '${tool.title}'? This operation will modify the document.") },
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
