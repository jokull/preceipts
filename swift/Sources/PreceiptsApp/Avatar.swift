// GitHub avatars for the thread area: a tiny async loader (NSCache +
// in-flight dedupe, URLCache-backed) and a circular avatar view with an
// initial-letter placeholder so cards never jump when images land.

import AppKit

final class AvatarStore {
    static let shared = AvatarStore()

    private let cache = NSCache<NSString, NSImage>()
    private var inFlight: [String: [(NSImage?) -> Void]] = [:]
    private let session: URLSession = {
        let configuration = URLSessionConfiguration.default
        configuration.requestCachePolicy = .returnCacheDataElseLoad
        return URLSession(configuration: configuration)
    }()

    /// Completion on the main thread; instantly when cached.
    func image(for urlString: String, completion: @escaping (NSImage?) -> Void) {
        if let hit = cache.object(forKey: urlString as NSString) {
            completion(hit)
            return
        }
        guard let url = URL(string: urlString) else {
            completion(nil)
            return
        }
        if inFlight[urlString] != nil {
            inFlight[urlString]?.append(completion)
            return
        }
        inFlight[urlString] = [completion]
        session.dataTask(with: url) { [weak self] data, _, _ in
            let image = data.flatMap(NSImage.init(data:))
            DispatchQueue.main.async {
                guard let self else { return }
                if let image {
                    self.cache.setObject(image, forKey: urlString as NSString)
                }
                self.inFlight.removeValue(forKey: urlString)?.forEach { $0(image) }
            }
        }.resume()
    }
}

/// Circular avatar. Shows a colored initial until (unless) the image
/// loads; the color is stable per login so authors stay recognizable.
final class AvatarView: NSView {
    private let diameter: CGFloat
    private var login: String = ""
    private var image: NSImage?

    init(diameter: CGFloat) {
        self.diameter = diameter
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: diameter),
            heightAnchor.constraint(equalToConstant: diameter),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func load(login: String, urlString: String?) {
        self.login = login
        self.image = nil
        needsDisplay = true
        guard let urlString else { return }
        AvatarStore.shared.image(for: urlString) { [weak self] image in
            guard let self, self.login == login else { return }
            self.image = image
            self.needsDisplay = true
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSBezierPath(ovalIn: bounds).addClip()
        if let image {
            image.draw(in: bounds, from: .zero, operation: .sourceOver, fraction: 1)
            return
        }
        Self.placeholderColor(login).setFill()
        bounds.fill()
        let initial = String(login.prefix(1)).uppercased()
        let font = NSFont.systemFont(ofSize: diameter * 0.5, weight: .semibold)
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: NSColor.white,
        ]
        let size = initial.size(withAttributes: attributes)
        initial.draw(
            at: NSPoint(
                x: (bounds.width - size.width) / 2,
                y: (bounds.height - size.height) / 2),
            withAttributes: attributes)
    }

    private static func placeholderColor(_ login: String) -> NSColor {
        var hash: UInt64 = 5381
        for byte in login.utf8 {
            hash = hash &* 33 &+ UInt64(byte)
        }
        let hue = CGFloat(hash % 360) / 360
        return NSColor(hue: hue, saturation: 0.45, brightness: 0.6, alpha: 1)
    }
}
