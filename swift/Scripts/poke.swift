// preceipts debug poker: posts a command to the app's DebugBridge.
// Usage: poke shot /path/out.png | poke feedback | poke thread 0 | poke conversation
import Foundation

let command = CommandLine.arguments.dropFirst().joined(separator: " ")
guard !command.isEmpty else {
    FileHandle.standardError.write(Data("usage: poke <command>\n".utf8))
    exit(1)
}
DistributedNotificationCenter.default().postNotificationName(
    Notification.Name("is.solberg.preceipts.debug"),
    object: command, userInfo: nil, deliverImmediately: true)
// Give the notification daemon a beat before exiting.
RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
