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

/// Where to relay paired devices found on this device's network.
struct RelayArgs: Decodable {
  /// The server's `/api/relay/devices` WebSocket URL.
  let controlUrl: String
  /// The server's `/api/relay/stream/` WebSocket URL; the stream id follows.
  let streamUrl: String
  let token: String
}

/// The WebSockets to the server. Each relayed device connection is one, and
/// URLSession.shared allows only a few at a time to one host.
let relaySession: URLSession = {
  let configuration = URLSessionConfiguration.default
  configuration.httpMaximumConnectionsPerHost = 64
  return URLSession(configuration: configuration)
}()

/// Pipes one device connection to the server's relay WebSocket, byte for byte,
/// until either side ends.
final class Relay {
  private let device: NWConnection
  private let socket: URLSessionWebSocketTask
  private let lock = NSLock()
  private var ended = false

  init(device: NWConnection, url: URL, token: String, queue: DispatchQueue) {
    self.device = device
    socket = relaySession.webSocketTask(with: url)
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
        self?.pumpSocket()
      case .failed, .cancelled:
        self?.end()
      default:
        break
      }
    }
    device.start(queue: queue)
  }

  var isEnded: Bool {
    lock.lock()
    defer { lock.unlock() }
    return ended
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

/// Finds paired devices on this device's network (they announce
/// `_remotepairing._tcp`), reports them to the server, and opens the
/// connections to them the server asks for. Reconnects while running.
final class DeviceRelay {
  private let args: RelayArgs
  private let queue: DispatchQueue
  private var socket: URLSessionWebSocketTask?
  private var browser: NWBrowser?
  /// Bonjour service name to the address it resolved to.
  private var found: [String: (host: String, port: UInt16)] = [:]
  /// Bonjour service name to its `identifier` and `authTag` TXT values, by
  /// which the server tells the devices paired with it from the others.
  private var txt: [String: (identifier: String, authTag: String)] = [:]
  private var resolving: [String: NWConnection] = [:]
  private var relays: [Relay] = []
  private var report: DispatchSourceTimer?
  private var running = true

  init(args: RelayArgs, queue: DispatchQueue) {
    self.args = args
    self.queue = queue
    queue.async {
      self.browse()
      self.connect()
      // Again now and then, so the server retries devices that failed to open.
      let timer = DispatchSource.makeTimerSource(queue: queue)
      timer.schedule(deadline: .now() + 30, repeating: 30)
      timer.setEventHandler { [weak self] in self?.sendDevices() }
      timer.resume()
      self.report = timer
    }
  }

  /// Runs on `queue`.
  func stop() {
    running = false
    report?.cancel()
    browser?.cancel()
    resolving.values.forEach { $0.cancel() }
    resolving.removeAll()
    relays.forEach { $0.end() }
    relays.removeAll()
    socket?.cancel(with: .goingAway, reason: nil)
    socket = nil
  }

  private func connect() {
    guard running, let url = URL(string: args.controlUrl) else { return }
    let socket = relaySession.webSocketTask(with: url)
    self.socket = socket
    socket.resume()
    send(["token": args.token])
    sendDevices()
    receive(socket)
  }

  private func receive(_ socket: URLSessionWebSocketTask) {
    socket.receive { [weak self] result in
      guard let self = self else { return }
      self.queue.async {
        guard socket === self.socket else { return }
        switch result {
        case .success(.string(let text)):
          self.handle(text)
          self.receive(socket)
        case .success:
          self.receive(socket)
        case .failure:
          // The server restarted or the network changed: try again shortly.
          self.socket = nil
          self.queue.asyncAfter(deadline: .now() + 3) { self.connect() }
        }
      }
    }
  }

  private func handle(_ text: String) {
    guard
      let data = text.data(using: .utf8),
      let op = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      op["type"] as? String == "open",
      let id = op["id"] as? String,
      let host = op["host"] as? String,
      let port = (op["port"] as? NSNumber)?.uint16Value,
      let nwPort = NWEndpoint.Port(rawValue: port),
      let url = URL(string: args.streamUrl + id)
    else { return }
    relays.removeAll { $0.isEnded }
    let device = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: .tcp)
    relays.append(Relay(device: device, url: url, token: args.token, queue: queue))
  }

  private func send(_ object: [String: Any]) {
    guard
      let socket = socket,
      let data = try? JSONSerialization.data(withJSONObject: object),
      let text = String(data: data, encoding: .utf8)
    else { return }
    socket.send(.string(text)) { _ in }
  }

  private func sendDevices() {
    let devices = found.compactMap { entry -> [String: Any]? in
      let (name, address) = (entry.key, entry.value)
      guard let meta = txt[name] else { return nil }
      return [
        "name": name, "host": address.host, "port": Int(address.port),
        "identifier": meta.identifier, "authTag": meta.authTag,
      ]
    }
    send(["type": "devices", "devices": devices])
  }

  private func browse() {
    let browser = NWBrowser(
      for: .bonjourWithTXTRecord(type: "_remotepairing._tcp", domain: nil), using: .tcp)
    browser.browseResultsChangedHandler = { [weak self] results, _ in
      guard let self = self else { return }
      var names = Set<String>()
      var changed = false
      for result in results {
        guard case .service(let name, _, _, _) = result.endpoint else { continue }
        // Only advertisements a paired host can recognize are of use.
        guard
          case .bonjour(let record) = result.metadata,
          let identifier = record["identifier"],
          let authTag = record["authTag"]
        else { continue }
        names.insert(name)
        if self.txt[name]?.identifier != identifier || self.txt[name]?.authTag != authTag {
          self.txt[name] = (identifier, authTag)
          changed = true
        }
        if self.found[name] == nil && self.resolving[name] == nil {
          self.resolve(name: name, endpoint: result.endpoint)
        }
      }
      let gone = self.txt.keys.filter { !names.contains($0) }
      gone.forEach {
        self.txt.removeValue(forKey: $0)
        self.found.removeValue(forKey: $0)
      }
      if changed || !gone.isEmpty { self.sendDevices() }
    }
    browser.stateUpdateHandler = { [weak self] state in
      // Local Network access was refused, or the network went away: retry.
      if case .failed = state, let self = self, self.running {
        self.queue.asyncAfter(deadline: .now() + 5) { self.browse() }
      }
    }
    self.browser = browser
    browser.start(queue: queue)
  }

  /// Connects once to learn the service's address and port, which the
  /// server sends back in its `open` requests.
  private func resolve(name: String, endpoint: NWEndpoint) {
    let parameters = NWParameters.tcp
    if let ip = parameters.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options {
      ip.version = .v4
    }
    let connection = NWConnection(to: endpoint, using: parameters)
    resolving[name] = connection
    connection.stateUpdateHandler = { [weak self] state in
      guard let self = self else { return }
      switch state {
      case .ready:
        if case .hostPort(let host, let port) = connection.currentPath?.remoteEndpoint {
          self.found[name] = (DeviceRelay.describe(host), port.rawValue)
          self.sendDevices()
        }
        connection.cancel()
        self.resolving.removeValue(forKey: name)
      case .failed, .waiting:
        connection.cancel()
        self.resolving.removeValue(forKey: name)
      default:
        break
      }
    }
    connection.start(queue: queue)
  }

  private static func describe(_ host: NWEndpoint.Host) -> String {
    switch host {
    case .ipv4(let address):
      return "\(address)"
    case .ipv6(let address):
      return "\(address)"
    case .name(let name, _):
      return name
    @unknown default:
      return "\(host)"
    }
  }
}

class OrchardPairingPlugin: Plugin {
  private let queue = DispatchQueue(label: "orchard.pairing")
  private var listener: NWListener?
  private var relays: [Relay] = []
  private var deviceRelay: DeviceRelay?
  /// Kept while a pairing pauses the device relay, to start it again after.
  private var relayArgs: RelayArgs?

  /// Announces the pairing host on this device's network and relays every
  /// connection a device opens to it. Resolves with the port once announced.
  @objc public func announce(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(AnnounceArgs.self)
    guard let url = URL(string: args.relayUrl) else {
      invoke.reject("The relay URL is not valid")
      return
    }
    // A device that is pairing drops the pairing when another connection
    // reaches its remote pairing service, so pause the device relay.
    queue.sync {
      stopAll()
      deviceRelay?.stop()
      deviceRelay = nil
    }

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
    // A pairing uses one connection: relay only the first and refuse the rest,
    // so nothing else on the network can take the session's single attempt.
    // Keep announcing until `stop`, though: the device watches the Bonjour
    // service while it pairs and aborts when it disappears.
    listener.newConnectionHandler = { [weak self] connection in
      guard let self = self, self.relays.isEmpty else {
        connection.cancel()
        return
      }
      self.relays.append(
        Relay(device: connection, url: url, token: args.token, queue: self.queue))
    }
    listener.start(queue: queue)
    queue.sync { self.listener = listener }
  }

  /// Reports paired devices on this network to the server and relays its
  /// connections to them, until `stopDeviceRelay`.
  @objc public func startDeviceRelay(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(RelayArgs.self)
    queue.sync {
      relayArgs = args
      deviceRelay?.stop()
      deviceRelay = DeviceRelay(args: args, queue: queue)
    }
    invoke.resolve()
  }

  @objc public func stopDeviceRelay(_ invoke: Invoke) {
    queue.sync {
      relayArgs = nil
      deviceRelay?.stop()
      deviceRelay = nil
    }
    invoke.resolve()
  }

  /// Stops announcing and ends every relay, then resumes the device relay.
  @objc public func stop(_ invoke: Invoke) {
    queue.sync {
      stopAll()
      if deviceRelay == nil, let args = relayArgs {
        deviceRelay = DeviceRelay(args: args, queue: queue)
      }
    }
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
