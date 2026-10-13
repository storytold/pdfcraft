import Foundation
import CoreGraphics

#if canImport(UIKit)
import UIKit

/// High-performance UIScrollView hosting a PdfPageView with native gestures:
/// pinch-to-zoom (25% to 1600%), pan, momentum, and double-tap zoom.
public final class PdfScrollView: UIScrollView, UIScrollViewDelegate {
    public private(set) var pageView: PdfPageView
    private var doubleTapGesture: UITapGestureRecognizer?

    public init(document: PdfDocument? = nil, pageIndex: Int = 0) {
        self.pageView = PdfPageView(document: document, pageIndex: pageIndex)
        super.init(frame: .zero)
        setupScrollView()
    }

    required init?(coder: NSCoder) {
        self.pageView = PdfPageView()
        super.init(coder: coder)
        setupScrollView()
    }

    private func setupScrollView() {
        delegate = self
        minimumZoomScale = 0.25   // 25%
        maximumZoomScale = 16.0   // 1600%
        bouncesZoom = true
        showsHorizontalScrollIndicator = true
        showsVerticalScrollIndicator = true
        backgroundColor = UIColor.systemGroupedBackground
        decelerationRate = .normal

        addSubview(pageView)

        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(handleDoubleTap(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
        self.doubleTapGesture = doubleTap
    }

    public func update(document: PdfDocument, pageIndex: Int) {
        pageView.update(document: document, pageIndex: pageIndex)
        layoutPageView()
    }

    public override func layoutSubviews() {
        super.layoutSubviews()
        centerPageView()
    }

    public func layoutPageView() {
        guard let geom = pageView.pageGeometry else { return }
        let pageW = CGFloat(geom.width)
        let pageH = CGFloat(geom.height)
        pageView.frame = CGRect(x: 0, y: 0, width: pageW, height: pageH)
        contentSize = CGSize(width: pageW, height: pageH)
        centerPageView()
    }

    private func centerPageView() {
        let boundsSize = bounds.size
        var frameToCenter = pageView.frame

        if frameToCenter.size.width < boundsSize.width {
            frameToCenter.origin.x = (boundsSize.width - frameToCenter.size.width) / 2
        } else {
            frameToCenter.origin.x = 0
        }

        if frameToCenter.size.height < boundsSize.height {
            frameToCenter.origin.y = (boundsSize.height - frameToCenter.size.height) / 2
        } else {
            frameToCenter.origin.y = 0
        }

        pageView.frame = frameToCenter
    }

    // MARK: - UIScrollViewDelegate

    public func viewForZooming(in scrollView: UIScrollView) -> UIView? {
        pageView
    }

    public func scrollViewWillBeginZooming(_ scrollView: UIScrollView, with view: UIView?) {
        pageView.document?.advanceGenerationAndCancelStale()
    }

    public func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
        pageView.document?.advanceGenerationAndCancelStale()
    }

    public func scrollViewDidZoom(_ scrollView: UIScrollView) {
        centerPageView()
    }

    // MARK: - Gestures & Navigation

    @objc private func handleDoubleTap(_ gesture: UITapGestureRecognizer) {
        if zoomScale > minimumZoomScale * 1.5 {
            setZoomScale(minimumZoomScale, animated: true)
        } else {
            let point = gesture.location(in: pageView)
            let targetScale: CGFloat = min(2.5, maximumZoomScale)
            let w = bounds.size.width / targetScale
            let h = bounds.size.height / targetScale
            let rectToZoom = CGRect(x: point.x - w / 2, y: point.y - h / 2, width: w, height: h)
            zoom(to: rectToZoom, animated: true)
        }
    }

    /// Zooms to fit page width within scroll view bounds.
    public func zoomToFitWidth(animated: Bool = true) {
        guard let geom = pageView.pageGeometry, geom.width > 0 else { return }
        let scale = bounds.width / CGFloat(geom.width)
        setZoomScale(min(max(scale, minimumZoomScale), maximumZoomScale), animated: animated)
    }

    /// Zooms to fit entire page within scroll view bounds.
    public func zoomToFitPage(animated: Bool = true) {
        guard let geom = pageView.pageGeometry, geom.width > 0, geom.height > 0 else { return }
        let scaleW = bounds.width / CGFloat(geom.width)
        let scaleH = bounds.height / CGFloat(geom.height)
        let scale = min(scaleW, scaleH)
        setZoomScale(min(max(scale, minimumZoomScale), maximumZoomScale), animated: animated)
    }
}
#endif
