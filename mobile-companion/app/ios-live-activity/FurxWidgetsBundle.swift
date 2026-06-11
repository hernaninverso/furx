import ActivityKit
import SwiftUI
import WidgetKit

// The FurxWidgets app-extension entry point. Renders the Live Activity on the
// Lock Screen + Dynamic Island. Estética "Atelier Terminal": tinta cálida + coral
// (migrado del teal/ink-frío viejo). Fuente del SISTEMA (no Fraunces) por legibilidad
// en Lock Screen — la identidad va por color + la marca "F" coral (consejo iOS).
extension Color {
  // Paleta de marca Furx (warm dark + coral). El coral del widget va ~+luminancia
  // respecto del #ff8a6e del desktop para contraste en pantallas móviles/OLED.
  static let furxInk = Color(red: 0.086, green: 0.075, blue: 0.059)     // #16130F fondo
  static let furxSurface = Color(red: 0.129, green: 0.110, blue: 0.090) // #211C17 superficie
  static let furxPaper = Color(red: 0.949, green: 0.910, blue: 0.847)   // #F2E8D8 texto
  static let furxCoral = Color(red: 1.0, green: 0.541, blue: 0.431)     // #FF8A6E acento / marca
  static let furxCoralHi = Color(red: 1.0, green: 0.627, blue: 0.533)   // coral +luminancia (texto chico)
}

@main
struct FurxWidgetsBundle: WidgetBundle {
  var body: some Widget {
    if #available(iOS 16.1, *) {
      FurxLiveActivityWidget()
    }
  }
}

@available(iOS 16.1, *)
struct FurxLiveActivityWidget: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: FurxActivityAttributes.self) { context in
      // Lock Screen / banner presentation.
      HStack(spacing: 10) {
        Text("F")
          .font(.system(size: 18, weight: .semibold, design: .serif))
          .italic()
          .foregroundColor(.furxCoral)
          .frame(width: 28, height: 28)
          .background(Color.furxSurface)
          .cornerRadius(8)
        VStack(alignment: .leading, spacing: 2) {
          Text("Furx").font(.caption).bold().foregroundColor(.furxPaper)
          Text(context.state.status).font(.footnote)
            .foregroundColor(.furxCoralHi)
        }
        Spacer()
      }
      .padding(12)
      .activityBackgroundTint(Color.furxInk)
      .activitySystemActionForegroundColor(Color.furxCoral)
    } dynamicIsland: { context in
      DynamicIsland {
        DynamicIslandExpandedRegion(.leading) {
          Text("Furx").font(.caption).bold().foregroundColor(.furxPaper)
        }
        DynamicIslandExpandedRegion(.center) {
          Text(context.state.status).font(.footnote).foregroundColor(.furxCoralHi)
        }
      } compactLeading: {
        Text("F").font(.system(size: 13, weight: .semibold, design: .serif)).italic().foregroundColor(.furxCoral)
      } compactTrailing: {
        Image(systemName: "waveform").foregroundColor(.furxCoral)
      } minimal: {
        Text("F").font(.system(size: 12, weight: .semibold, design: .serif)).italic().foregroundColor(.furxCoral)
      }
      .widgetURL(URL(string: "furx://pane/\(context.attributes.paneId)"))
    }
  }
}
