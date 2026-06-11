import ActivityKit
import Foundation

// SHARED between the app target and the FurxWidgets extension (added to BOTH
// targets' membership by add_widget_target.rb). ActivityKit matches the running
// activity to the widget by this attributes type across the app↔extension pair.
@available(iOS 16.1, *)
public struct FurxActivityAttributes: ActivityAttributes {
  public struct ContentState: Codable, Hashable {
    // Short, NON-sensitive status line (F-IV: no pane content).
    public var status: String
    public init(status: String) { self.status = status }
  }

  // Which pane this activity tracks (id only; not content).
  public var paneId: String
  public init(paneId: String) { self.paneId = paneId }
}
