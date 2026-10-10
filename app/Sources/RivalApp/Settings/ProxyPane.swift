import RivalKit
import SwiftUI

/// The Proxy tab: the two route switches, the URL with its status dot, the
/// key, a model prefix per provider and where each model's requests go.
struct ProxyPane: View {
    @Bindable var settings: SettingsModel
    @State private var keyInput = ""

    var body: some View {
        CLIGate(settings: settings) {
            Form {
                Section {
                    Toggle("Claude through proxy", isOn: settings.boolBinding("proxy.claude.enabled"))
                    Toggle("Codex through proxy", isOn: settings.boolBinding("proxy.codex.enabled"))
                } footer: {
                    Text("Opus and Fable use the Claude route; Codex and Sol the Codex route.")
                        .foregroundStyle(.secondary)
                }

                Section("Proxy") {
                    TextField("URL", text: settings.stringBinding("proxy.url"), prompt: Text("http://127.0.0.1:8317"))
                        .font(.system(.body, design: .monospaced))
                    LabeledContent("Status") {
                        HStack(spacing: 6) {
                            ProxyStatusDot(status: settings.proxy)
                            Button {
                                Task { await settings.refreshProxy() }
                            } label: {
                                Image(systemName: "arrow.clockwise")
                            }
                            .buttonStyle(.borderless)
                            .help("Ask the proxy again (saved URL and key)")
                        }
                    }
                    LabeledContent("Key") {
                        HStack {
                            SecureField("Key", text: $keyInput, prompt: Text("paste the proxy key"))
                                .labelsHidden()
                                .onSubmit(saveKey)
                            Button("Save key", action: saveKey)
                                .disabled(keyInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || settings.busy)
                        }
                    }
                    LabeledContent("") {
                        HStack {
                            KeyStateLabel(show: settings.saved)
                            Spacer()
                            if settings.saved?.keySet == true, settings.saved?.source("proxy.key") == "file" {
                                Button("Remove", role: .destructive) { Task { await settings.clearKey() } }
                                    .buttonStyle(.borderless)
                                    .disabled(settings.busy)
                            }
                        }
                    }
                }

                Section("Model prefix") {
                    PrefixPicker(settings: settings, provider: "claude", title: "Claude prefix")
                    PrefixPicker(settings: settings, provider: "codex", title: "Codex prefix")
                }

                Section("Requests go to") {
                    ForEach(ModelCatalog.all.filter { $0.provider != nil }) { entry in
                        LabeledContent {
                            HStack(spacing: 6) {
                                Text(settings.route(entry))
                                    .foregroundStyle(.secondary)
                                Image(systemName: "arrow.right")
                                    .foregroundStyle(.tertiary)
                                Text(settings.wire(entry))
                                    .font(.system(.body, design: .monospaced))
                                    .textSelection(.enabled)
                            }
                        } label: {
                            Text(entry.title)
                        }
                    }
                }
            }
            .formStyle(.grouped)
            .safeAreaInset(edge: .bottom, spacing: 0) { ApplyBar(settings: settings) }
        }
    }

    private func saveKey() {
        let key = keyInput
        Task {
            if await settings.saveKey(key) {
                keyInput = ""
                await settings.refreshProxy()
            }
        }
    }
}

/// ● online · 38 models, ● offline + the error, or ○ not checked.
struct ProxyStatusDot: View {
    let status: SettingsModel.ProxyStatus

    var body: some View {
        switch status {
        case .unknown:
            dot(.secondary, "not checked")
        case .loading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("checking…").foregroundStyle(.secondary)
            }
        case .online(let models):
            dot(.green, "online · \(models.count) models")
        case .offline(let error):
            dot(.red, "offline")
                .help(error)
            Text(error)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
                .help(error)
        }
    }

    private func dot(_ color: Color, _ text: String) -> some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 8, height: 8)
            Text(text)
        }
    }
}

/// "Saved …a91f", "Saved (from RIVAL_PROXY_KEY)" or "No key saved".
struct KeyStateLabel: View {
    let show: ConfigShow?

    var body: some View {
        if let show, show.keySet {
            let tail = show.keyTail.map { " …\($0)" } ?? ""
            let env = show.source("proxy.key") == "env" ? " (from RIVAL_PROXY_KEY)" : ""
            Label("Saved\(tail)\(env)", systemImage: "key.fill")
                .foregroundStyle(.secondary)
        } else if show?.string("proxy.key") == "error" {
            Label("The key file cannot be read", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
        } else {
            Text("No key saved")
                .foregroundStyle(.secondary)
        }
    }
}

/// The prefix choices for one provider, filled from `rival config models`.
struct PrefixPicker: View {
    @Bindable var settings: SettingsModel
    let provider: String
    let title: String

    var body: some View {
        let key = "proxy.\(provider).model_prefix"
        Picker(title, selection: settings.stringBinding(key)) {
            ForEach(settings.prefixOptions(provider), id: \.self) { prefix in
                Text(prefix.isEmpty ? "none" : prefix)
                    .font(.system(.body, design: .monospaced))
                    .tag(prefix)
            }
        }
    }
}
