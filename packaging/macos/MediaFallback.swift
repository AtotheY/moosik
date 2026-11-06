import AppKit
import ServiceManagement

// This helper observes app launches, never keyboard events or Now Playing ownership.
// ServiceManagement owns its lifetime, including login and disabling it outside Moosik.
#if MEDIA_FALLBACK_TEST
let service = SMAppService.agent(plistName: "com.moosik.media-fallback.test.plist")
#else
let service = SMAppService.agent(plistName: "com.moosik.media-fallback.plist")
#endif

func printStatus() {
    switch service.status {
    case .notRegistered: print("off")
    case .enabled: print("on")
    case .requiresApproval: print("approval")
    // A never-registered bundled LaunchAgent can also report notFound. Registration is
    // still allowed; a missing/corrupt plist produces an actionable error from register().
    case .notFound: print("off")
    @unknown default: print("unknown")
    }
}

func isAppleMusic(_ identifier: String?) -> Bool {
    #if MEDIA_FALLBACK_TEST
    // Native integration tests use a disposable app, never the user's music/account.
    identifier == "com.moosik.media-fallback.fixture"
    #else
    identifier == "com.apple.Music" || identifier == "com.apple.iTunes"
    #endif
}

let arguments = CommandLine.arguments.dropFirst()
if let command = arguments.first {
    do {
        switch command {
        case "--status": break
        case "--enable":
            if service.status != .enabled && service.status != .requiresApproval {
                try service.register()
            }
        case "--disable":
            if service.status != .notRegistered && service.status != .notFound {
                try service.unregister()
            }
        case "--settings":
            SMAppService.openSystemSettingsLoginItems()
        default:
            throw NSError(domain: "Moosik", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Unknown media helper command."])
        }
        printStatus()
        exit(0)
    } catch {
        fputs("\(error.localizedDescription)\n", stderr)
        exit(1)
    }
}

// Derive the install location every time: moving/updating the containing app must not leave
// a hardcoded path to an older build. A helper outside a complete bundle does nothing.
// The monitor has its own nested bundle identity. Registering it as the main app would
// make LaunchServices mistake a background monitor for the user's player window.
let moosik = Bundle.main.bundleURL
    .deletingLastPathComponent() // Contents/Helpers
    .deletingLastPathComponent() // Contents
    .deletingLastPathComponent() // Moosik.app
guard moosik.pathExtension == "app",
      FileManager.default.isExecutableFile(atPath:
          moosik.appendingPathComponent("Contents/MacOS/moosik").path) else { exit(1) }

let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
var redirected = Set<pid_t>()
let center = NSWorkspace.shared.notificationCenter
var observers: [NSObjectProtocol] = []
for notification in [NSWorkspace.willLaunchApplicationNotification,
                     NSWorkspace.didLaunchApplicationNotification] {
    observers.append(center.addObserver(forName: notification, object: nil, queue: .main) { event in
        guard let target = event.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
              isAppleMusic(target.bundleIdentifier),
              !redirected.contains(target.processIdentifier),
              target.forceTerminate() else { return }
        // Both launch notifications can arrive for one process. Resume is idempotent too,
        // but avoid spawning duplicate short-lived Moosik instances in the first place.
        if redirected.count > 128 { redirected.removeAll() }
        redirected.insert(target.processIdentifier)
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = false
        configuration.createsNewApplicationInstance = true
        configuration.arguments = ["--resume"]
        NSWorkspace.shared.openApplication(at: moosik, configuration: configuration) { _, error in
            if let error { NSLog("Moosik media fallback: %@", error.localizedDescription) }
        }
    })
}
// Do not terminate Apple Music if it was already running when the setting was enabled.
application.run()
