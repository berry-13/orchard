import Foundation
import Network
import Tauri
import WebKit

/// What `start_wifi_pairing` on the server returned, plus where to relay.
struct AnnounceArgs: Decodable {
  /// `_remotepairing-pairable-host._tcp`
  let serviceType: String
  let serviceName: String
  let txt: [String: String]
  /// The server's `ws://` or `wss://` relay URL for this pairing session.
  let relayUrl: String
  /// The client's server token, sent as the first WebSocket message.
  let token: String
}

/// Pipes one device connection to the server's relay WebSocket, byte for byte,
/// until either side ends.
final class Relay {
  private let device: NWConnection
  private let socket: URLSessionWebSocketTask
  private let lock = NSLock()
  private var ended = false

  init(device: NWConnection, url: URL, token: String, queue: DispatchQueue) {
    self.device = device
    socket = URLSession.shared.webSocketTask(with: url)
    socket.resume()
    let hello = (try? JSONSerialization.data(withJSONObject: ["token": token]))
      .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    socket.send(.string(hello)) { [weak self] error in
      if error != nil { self?.end() }
    }
    device.stateUpdateHandler = { [weak self] state in
      switch state {
      case .ready:
        self?.pumpDevice()
      case .failed, .cancelled:
        self?.end()
      default:
        break
      }
    }
    device.start(queue: queue)
    pumpSocket()
  }

  private func pumpDevice() {
    device.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) {
      [weak self] data, _, isComplete, error in
      guard let self = self else { return }
      if let data = data, !data.isEmpty {
        self.socket.send(.data(data)) { error in
          if error != nil { self.end() }
        }
      }
      if isComplete || error != nil {
        self.end()
        return
      }
      self.pumpDevice()
    }
  }

  private func pumpSocket() {
    socket.receive { [weak self] result in
      guard let self = self else { return }
      switch result {
      case .success(let message):
        switch message {
        case .data(let data):
          self.device.send(
            content: data,
            completion: .contentProcessed { error in
              if error != nil { self.end() }
            })
        case .string:
          break
        @unknown default:
          break
        }
        self.pumpSocket()
      case .failure:
        self.end()
      }
    }
  }

  func end() {
    lock.lock()
    let first = !ended
    ended = true
    lock.unlock()
    guard first else { return }
    device.cancel()
    socket.cancel(with: .goingAway, reason: nil)
  }
}

class OrchardPairingPlugin: Plugin {
  private let queue = DispatchQueue(label: "orchard.pairing")
  private var listener: NWListener?
  private var relays: [Relay] = []

  /// Announces the pairing host on this device's network and relays every
  /// connection a device opens to it. Resolves with the port once announced.
  @objc public func announce(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(AnnounceArgs.self)
    guard let url = URL(string: args.relayUrl) else {
      invoke.reject("The relay URL is not valid")
      return
    }
    queue.sync { stopAll() }

    let listener: NWListener
    do {
      listener = try NWListener(using: .tcp)
    } catch {
      invoke.reject("Could not open a listening socket: \(error.localizedDescription)")
      return
    }
    var txt = NWTXTRecord()
    for (key, value) in args.txt {
      txt[key] = value
    }
    listener.service = NWListener.Service(
      name: args.serviceName, type: args.serviceType, domain: nil, txtRecord: txt)

    var answered = false
    listener.stateUpdateHandler = { [weak self, weak listener] state in
      switch state {
      case .ready:
        if !answered {
          answered = true
          invoke.resolve(["port": Int(listener?.port?.rawValue ?? 0)])
        }
      case .failed(let error):
        if !answered {
          answered = true
          invoke.reject(
            "Could not announce on the local network (is Local Network access allowed?): \(error)")
        }
        self?.stopAll()
      default:
        break
      }
    }
    // A pairing uses one connection. Relay only the first and stop announcing,
    // so nothing else on the network can take the session's single attempt.
    listener.newConnectionHandler = { [weak self, weak listener] connection in
      guard let self = self, self.relays.isEmpty else {
        connection.cancel()
        return
      }
      listener?.newConnectionHandler = nil
      listener?.cancel()
      self.relays.append(
        Relay(device: connection, url: url, token: args.token, queue: self.queue))
    }
    listener.start(queue: queue)
    queue.sync { self.listener = listener }
  }

  /// Stops announcing and ends every relay.
  @objc public func stop(_ invoke: Invoke) {
    queue.sync { stopAll() }
    invoke.resolve()
  }

  /// Runs on `queue`.
  private func stopAll() {
    listener?.cancel()
    listener = nil
    relays.forEach { $0.end() }
    relays.removeAll()
  }
}

@_cdecl("init_plugin_orchard_pairing")
func initPlugin() -> Plugin {
  return OrchardPairingPlugin()
}
