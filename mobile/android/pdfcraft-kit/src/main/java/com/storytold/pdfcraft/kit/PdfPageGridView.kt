package com.storytold.pdfcraft.kit

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Thumbnail grid view for visual page navigation, reordering, rotation,
 * and page deletion (Phase 5 / MOB-5.4).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfPageGridView(
    document: PdfDocument,
    onPageSelected: (Int) -> Unit,
    modifier: Modifier = Modifier
) {
    var pageCount by remember { mutableStateOf(document.pageCount) }
    var selectedPages by remember { mutableStateOf(setOf<Int>()) }
    var isSelectionMode by remember { mutableStateOf(false) }
    var showDeleteConfirm by remember { mutableStateOf(false) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    val thumbnails = remember { mutableStateMapOf<Int, Bitmap>() }

    fun reloadPages() {
        pageCount = document.pageCount
        thumbnails.clear()
        selectedPages = emptySet()
    }

    LaunchedEffect(document, pageCount) {
        withContext(Dispatchers.IO) {
            for (p in 0 until pageCount) {
                if (!thumbnails.containsKey(p)) {
                    val bmp = document.renderThumbnail(p, 200f)
                    if (bmp != null) {
                        thumbnails[p] = bmp
                    }
                }
            }
        }
    }

    Column(modifier = modifier.fillMaxSize()) {
        // Action Bar
        Surface(
            tonalElevation = 2.dp,
            modifier = Modifier.fillMaxWidth()
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                TextButton(onClick = {
                    isSelectionMode = !isSelectionMode
                    if (!isSelectionMode) selectedPages = emptySet()
                }) {
                    Text(if (isSelectionMode) "Done" else "Select")
                }

                if (isSelectionMode && selectedPages.isNotEmpty()) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = {
                            try {
                                for (p in selectedPages) {
                                    document.rotatePage(p, 90)
                                }
                                reloadPages()
                            } catch (e: Throwable) {
                                errorMessage = e.message
                            }
                        }) {
                            Text("Rotate")
                        }

                        Button(
                            onClick = { showDeleteConfirm = true },
                            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error)
                        ) {
                            Text("Delete")
                        }
                    }
                } else {
                    Button(onClick = {
                        try {
                            document.insertBlankPage(pageCount - 1)
                            reloadPages()
                        } catch (e: Throwable) {
                            errorMessage = e.message
                        }
                    }) {
                        Text("+ Blank Page")
                    }
                }
            }
        }

        errorMessage?.let { err ->
            Text(
                text = err,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(16.dp)
            )
        }

        // Thumbnails Grid
        LazyVerticalGrid(
            columns = GridCells.Adaptive(minSize = 130.dp),
            contentPadding = PaddingValues(16.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            modifier = Modifier.fillMaxSize()
        ) {
            items(pageCount) { pageIndex ->
                val isSelected = selectedPages.contains(pageIndex)
                val bmp = thumbnails[pageIndex]

                Card(
                    shape = RoundedCornerShape(8.dp),
                    elevation = CardDefaults.cardElevation(defaultElevation = if (isSelected) 6.dp else 2.dp),
                    colors = CardDefaults.cardColors(
                        containerColor = if (isSelected) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surface
                    ),
                    modifier = Modifier
                        .fillMaxWidth()
                        .aspectRatio(0.75f)
                        .then(
                            if (isSelected) Modifier.border(2.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(8.dp))
                            else Modifier
                        )
                        .clickable {
                            if (isSelectionMode) {
                                selectedPages = if (isSelected) selectedPages - pageIndex else selectedPages + pageIndex
                            } else {
                                onPageSelected(pageIndex)
                            }
                        }
                ) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        if (bmp != null) {
                            Image(
                                bitmap = bmp.asImageBitmap(),
                                contentDescription = "Page ${pageIndex + 1}",
                                contentScale = ContentScale.Fit,
                                modifier = Modifier
                                    .fillMaxSize()
                                    .padding(8.dp)
                                    .clip(RoundedCornerShape(4.dp))
                            )
                        } else {
                            Box(
                                modifier = Modifier.fillMaxSize(),
                                contentAlignment = Alignment.Center
                            ) {
                                CircularProgressIndicator(modifier = Modifier.size(24.dp))
                            }
                        }

                        // Page badge
                        Surface(
                            shape = RoundedCornerShape(bottomEnd = 8.dp),
                            color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.85f),
                            modifier = Modifier.align(Alignment.TopStart)
                        ) {
                            Text(
                                text = "${pageIndex + 1}",
                                style = MaterialTheme.typography.labelSmall,
                                modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp)
                            )
                        }
                    }
                }
            }
        }
    }

    if (showDeleteConfirm) {
        AlertDialog(
            onDismissRequest = { showDeleteConfirm = false },
            title = { Text("Delete Pages") },
            text = { Text("Are you sure you want to delete ${selectedPages.size} selected page(s)? This action can be undone.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        try {
                            // Delete in reverse order so indices remain valid
                            val sortedDesc = selectedPages.sortedDescending()
                            for (p in sortedDesc) {
                                document.deletePage(p)
                            }
                            showDeleteConfirm = false
                            reloadPages()
                        } catch (e: Throwable) {
                            errorMessage = e.message
                            showDeleteConfirm = false
                        }
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)
                ) {
                    Text("Delete")
                }
            },
            dismissButton = {
                TextButton(onClick = { showDeleteConfirm = false }) {
                    Text("Cancel")
                }
            }
        )
    }
}

