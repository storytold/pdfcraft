import QuartzCore

/// Specialized CATiledLayer for high-fidelity PDF page rendering.
/// Disables fade-in duration to eliminate flicker and configures multi-level LOD.
public final class PdfTiledLayer: CATiledLayer {
    public override class func fadeDuration() -> CFTimeInterval {
        return 0.0
    }

    public override init() {
        super.init()
        setupTiling()
    }

    public override init(layer: Any) {
        super.init(layer: layer)
        setupTiling()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setupTiling()
    }

    private func setupTiling() {
        // 512x512 tile size for optimal balance between memory, render time, and GPU texturing
        tileSize = CGSize(width: 512, height: 512)
        // Support zoom levels from 0.25x up to 16x (4 LOD levels)
        levelsOfDetail = 5
        levelsOfDetailBias = 4
    }
}
