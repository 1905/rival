import RivalKit
import SwiftUI

/// The Models tab: one row per reviewer model with its runtime, its route
/// (with the draft applied), its effort and whether `rival command plan`
/// uses it by default.
struct ModelsPane: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        CLIGate(settings: settings) {
            VStack(alignment: .leading, spacing: 0) {
                Table(ModelCatalog.all) {
                    TableColumn("Model") { entry in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(entry.title)
                            Text(settings.wire(entry))
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.secondary)
                        }
                    }
                    .width(min: 160, ideal: 200)
                    TableColumn("Runtime") { entry in
                        Text(entry.runtime)
                            .font(.system(.body, design: .monospaced))
                    }
                    .width(min: 70, ideal: 80)
                    TableColumn("Route") { entry in
                        let route = settings.route(entry)
                        Text(route)
                            .foregroundStyle(route == "proxy" ? Color.accentColor : Color.secondary)
                    }
                    .width(min: 50, ideal: 60)
                    TableColumn("Effort") { entry in
                        EffortPicker(settings: settings, entry: entry)
                    }
                    .width(min: 100, ideal: 110)
                    TableColumn("Plan default") { entry in
                        PlanToggle(settings: settings, entry: entry)
                    }
                    .width(min: 80, ideal: 90)
                }
                Text("Plan default: the models `rival command plan` runs without -m.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 8)
            }
            .safeAreaInset(edge: .bottom, spacing: 0) { ApplyBar(settings: settings) }
        }
    }
}

/// `efforts.<label>`. K3 has the one value `max`.
struct EffortPicker: View {
    @Bindable var settings: SettingsModel
    let entry: ModelEntry

    var body: some View {
        let key = "efforts.\(entry.id)"
        let current = settings.string(key)
        // Keep an unknown saved value selectable rather than blank.
        let options = entry.efforts.contains(current) || current.isEmpty ? entry.efforts : entry.efforts + [current]
        Picker("Effort", selection: settings.stringBinding(key)) {
            ForEach(options, id: \.self) { Text($0).tag($0) }
        }
        .labelsHidden()
        .disabled(options.count < 2)
    }
}

/// Membership in `plan.models`. The last model cannot be turned off: an
/// empty list would fall back to the built-in default.
struct PlanToggle: View {
    @Bindable var settings: SettingsModel
    let entry: ModelEntry

    var body: some View {
        if let name = entry.planName {
            let models = ModelCatalog.planNames(settings.list("plan.models"))
            let on = models.contains(name)
            Toggle("Plan default", isOn: Binding(
                get: { on },
                set: { want in
                    let next = want ? models + [name] : models.filter { $0 != name }
                    let ordered = ModelCatalog.planNames(next)
                    settings.set("plan.models", .array(ordered.map { .string($0) }))
                }
            ))
            .labelsHidden()
            .toggleStyle(.checkbox)
            .disabled(on && models.count == 1)
        } else {
            Text("—").foregroundStyle(.tertiary)
        }
    }
}
