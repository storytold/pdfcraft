package io.github.dikshant363.pdfcraft.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R

/** Inter (OFL, assets/fonts), PdfCraft's UI face, copied in by the build. */
val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_medium, FontWeight.Medium),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
)

/** The colours the screens use beyond Material's roles (measured from the reference app's screens). */
@Immutable
data class PdfColors(
    val dark: Boolean,
    /** The app's surfaces: screens, bars and sheets. */
    val surface: Color,
    /** Slightly raised: cards, the search field, selected rows. */
    val surfaceRaised: Color,
    /** A pressed or selected control's background (the active tool, an open picker's button). */
    val pressed: Color,
    /** The viewer's desk around and between the pages. */
    val desk: Color,
    val pageEdge: Color,
    val text: Color,
    val textSecondary: Color,
    /** Dates and other quiet text (the comment list's dates). */
    val textTertiary: Color,
    /** Bar and sheet icons. */
    val icon: Color,
    val divider: Color,
    /** A sheet's drag handle. */
    val handle: Color,
    /** Selection, the active tool, checked rows. */
    val accent: Color,
    val onAccent: Color,
    /** Filled buttons (primary actions). */
    val button: Color,
    /** The PDF badge and destructive actions. */
    val red: Color,
    val scrim: Color,
    /** Hint toasts over the page. */
    val toast: Color,
    val onToast: Color,
    /** Labels of the viewer's bars (the reference's primary label colour). */
    val label: Color,
    /** The corner triangle of a tool with sub-tools (the same in both themes). */
    val cornerMark: Color = Color(0xFF4B4B4B),
    /** The active tool's icon and the Done check (the reference's darker blue on its bars). */
    val toolAccent: Color,
    /** The comment panel's "N of M" and its reply box's outline and hint. */
    val navigatorText: Color,
    val replyBorder: Color,
    /** Home, Files, Tools and Search: the reference's home background. */
    val home: Color,
    /** The card behind each page of Pages. */
    val pageCard: Color,
    /** Pop-up menus over the page (the text selection's). */
    val menu: Color,
    /** The band behind the navigation buttons in the viewer and its screens and sheets. */
    val navBand: Color,
    /** The Read aloud switch's track, off and on. */
    val switchOff: Color,
    val switchOn: Color,
)

val LightPdfColors = PdfColors(
    dark = false,
    surface = Color(0xFFFFFFFF),
    surfaceRaised = Color(0xFFF4F4F4),
    pressed = Color(0xFFE9E9E9),
    desk = Color(0xFFE1E1E1),
    pageEdge = Color(0x1F000000),
    text = Color(0xFF202020),
    textSecondary = Color(0xFF5E5E5E),
    textTertiary = Color(0xFF6D6D6D),
    icon = Color(0xFF222222),
    divider = Color(0xFFD5D5D5),
    handle = Color(0xFF909090),
    accent = Color(0xFF147AF3),
    onAccent = Color(0xFFFFFFFF),
    button = Color(0xFF0265DC),
    red = Color(0xFFD7373F),
    scrim = Color(0x66000000),
    toast = Color(0xFF474747),
    onToast = Color(0xFFFFFFFF),
    label = Color(0xFF000000),
    toolAccent = Color(0xFF0265DC),
    navBand = Color(0xFFFFFFFF),
    menu = Color(0xFFFFFFFF),
    pageCard = Color(0xFFE6E6E6),
    home = Color(0xFFFFFFFF),
    navigatorText = Color(0xFF2C2C2C),
    replyBorder = Color(0xFF464646),
    switchOff = Color(0xFFE9E9E9),
    switchOn = Color(0xFF3B63FB),
)

val DarkPdfColors = PdfColors(
    dark = true,
    surface = Color(0xFF1D1D1D),
    surfaceRaised = Color(0xFF2A2A2A),
    pressed = Color(0xFF303030),
    desk = Color(0xFF000000),
    pageEdge = Color(0x00000000),
    text = Color(0xFFFFFFFF),
    textSecondary = Color(0xFFB2B2B2),
    textTertiary = Color(0xFFB0B0B0),
    icon = Color(0xFFEBEBEB),
    divider = Color(0xFF4B4B4B),
    handle = Color(0xFF8D8D8D),
    accent = Color(0xFF5EAAF7),
    onAccent = Color(0xFF000000),
    button = Color(0xFF0265DC),
    red = Color(0xFFF2545B),
    scrim = Color(0x99000000),
    toast = Color(0xFF474747),
    onToast = Color(0xFFFFFFFF),
    label = Color(0xFFFFFFFF),
    toolAccent = Color(0xFF5EAAF7),
    navBand = Color(0xFF080808),
    menu = Color(0xFF080808),
    pageCard = Color(0xFF303030),
    home = Color(0xFF000000),
    navigatorText = Color(0xFFEFEFEF),
    replyBorder = Color(0xFFD1D1D1),
    switchOff = Color(0xFF505050),
    switchOn = Color(0xFF4B75FF),
)

val LocalPdfColors = staticCompositionLocalOf { LightPdfColors }

/** The colours in effect. */
val pdfColors: PdfColors @Composable get() = LocalPdfColors.current

/** How the app chooses light or dark (Preferences ▸ Theme). */
enum class ThemeChoice { System, Light, Dark }

@Composable
fun PdfcraftTheme(choice: ThemeChoice = ThemeChoice.System, content: @Composable () -> Unit) {
    val dark = when (choice) {
        ThemeChoice.System -> isSystemInDarkTheme()
        ThemeChoice.Light -> false
        ThemeChoice.Dark -> true
    }
    val c = if (dark) DarkPdfColors else LightPdfColors
    val scheme: ColorScheme = if (dark) {
        darkColorScheme(primary = c.accent, onPrimary = c.onAccent, surface = c.surface, onSurface = c.text, background = c.surface, onBackground = c.text,
            surfaceVariant = c.surfaceRaised, onSurfaceVariant = c.textSecondary, outline = c.divider, error = c.red, surfaceContainer = c.surfaceRaised,
            surfaceContainerHigh = c.surfaceRaised, surfaceContainerHighest = c.surfaceRaised, surfaceContainerLow = c.surface)
    } else {
        lightColorScheme(primary = c.accent, onPrimary = c.onAccent, surface = c.surface, onSurface = c.text, background = c.surface, onBackground = c.text,
            surfaceVariant = c.surfaceRaised, onSurfaceVariant = c.textSecondary, outline = c.divider, error = c.red, surfaceContainer = c.surface,
            surfaceContainerHigh = c.surface, surfaceContainerHighest = c.surfaceRaised, surfaceContainerLow = c.surface)
    }
    val base = Typography()
    val typography = Typography(
        displayLarge = base.displayLarge.copy(fontFamily = Inter),
        headlineMedium = base.headlineMedium.copy(fontFamily = Inter),
        headlineSmall = base.headlineSmall.copy(fontFamily = Inter),
        titleLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 20.sp, lineHeight = 26.sp),
        titleMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, lineHeight = 22.sp),
        titleSmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 20.sp),
        bodyLarge = TextStyle(fontFamily = Inter, fontSize = 16.sp, lineHeight = 22.sp),
        bodyMedium = TextStyle(fontFamily = Inter, fontSize = 14.sp, lineHeight = 20.sp),
        bodySmall = TextStyle(fontFamily = Inter, fontSize = 12.sp, lineHeight = 16.sp),
        labelLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 14.sp, lineHeight = 20.sp),
        labelMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 12.sp, lineHeight = 16.sp),
        labelSmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Medium, fontSize = 11.sp, lineHeight = 14.sp),
    )
    CompositionLocalProvider(LocalPdfColors provides c) {
        MaterialTheme(colorScheme = scheme, typography = typography, content = content)
    }
}
