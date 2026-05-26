import Foundation

#if canImport(Sparkle)
  import Sparkle
#endif

enum InAppUpdateRuntimeState: String {
  case disabledByManagedPolicy = "disabled_by_managed_policy"
  case sparkleUnavailable = "sparkle_unavailable"
  case starting = "starting"
}

protocol InAppUpdateRuntime {
  func startIfAllowed() -> InAppUpdateRuntimeState
}

final class APWInAppUpdateRuntime: InAppUpdateRuntime {
  private let defaults: UserDefaults

  init(
    defaults: UserDefaults = .standard
  ) {
    self.defaults = defaults
  }

  func startIfAllowed() -> InAppUpdateRuntimeState {
    guard !managedUpdatesDisabled(defaults: defaults) else {
      return .disabledByManagedPolicy
    }

    #if canImport(Sparkle)
      SparkleRuntimeController.shared.start()
      return .starting
    #else
      return .sparkleUnavailable
    #endif
  }
}

#if canImport(Sparkle)
  private final class SparkleRuntimeController {
    static let shared = SparkleRuntimeController()

    @MainActor private var updaterController: SPUStandardUpdaterController?

    private init() {}

    func start() {
      if Thread.isMainThread {
        MainActor.assumeIsolated {
          self.startOnMainActor()
        }
      } else {
        DispatchQueue.main.async {
          self.startOnMainActor()
        }
      }
    }

    @MainActor private func startOnMainActor() {
      if updaterController == nil {
        updaterController = SPUStandardUpdaterController(
          startingUpdater: true,
          updaterDelegate: nil,
          userDriverDelegate: nil
        )
      }
    }
  }
#endif
