package io.github.dikshant363.pdfcraft.ui

/** A full-screen tool shown over the app. */
sealed class ToolScreen {
    class Compress(val doc: OpenDocument) : ToolScreen()
    class Combine(val doc: OpenDocument?) : ToolScreen()
    class Protect(val doc: OpenDocument) : ToolScreen()
    data object Create : ToolScreen()
    class Crop(val doc: OpenDocument) : ToolScreen()
    class Export(val doc: OpenDocument) : ToolScreen()
    data object Scan : ToolScreen()
}
