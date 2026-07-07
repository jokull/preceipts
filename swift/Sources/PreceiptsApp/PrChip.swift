// The titlebar's PR association: "#123 title · review · CI" — visible
// but quiet, click opens the PR. Absent entirely when there's no PR or
// no GitHub data (local-only degradation is by design).

import PreceiptsKit
import SwiftUI

@MainActor
final class PrChipModel: ObservableObject {
    @Published var status: PrStatus?
}

struct PrChipView: View {
    @ObservedObject var model: PrChipModel

    var body: some View {
        if let status = model.status {
            Button {
                if let url = URL(string: status.url) {
                    NSWorkspace.shared.open(url)
                }
            } label: {
                HStack(spacing: 5) {
                    Text("#\(status.number)")
                        .fontWeight(.semibold)
                    Text(status.title)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .frame(maxWidth: 240, alignment: .leading)
                        .foregroundStyle(.secondary)
                    if let review = status.reviewDecision {
                        reviewIcon(review)
                    }
                    if let ci = status.ciState {
                        ciIcon(ci)
                    }
                }
                .font(.callout)
            }
            .buttonStyle(.plain)
            .help("\(status.title) \u{2014} open on GitHub")
        } else {
            // Non-zero placeholder so the toolbar can measure the item.
            Color.clear.frame(width: 1, height: 1)
        }
    }

    @ViewBuilder private func reviewIcon(_ decision: PrStatus.ReviewDecision) -> some View {
        switch decision {
        case .approved:
            Image(systemName: "checkmark.seal.fill")
                .foregroundStyle(.green)
                .help("Approved")
        case .changesRequested:
            Image(systemName: "exclamationmark.octagon.fill")
                .foregroundStyle(.red)
                .help("Changes requested")
        case .reviewRequired:
            Image(systemName: "hourglass.circle")
                .foregroundStyle(.secondary)
                .help("Review required")
        }
    }

    @ViewBuilder private func ciIcon(_ state: PrStatus.CiState) -> some View {
        switch state {
        case .success:
            Image(systemName: "checkmark.circle.fill")
                .foregroundStyle(.green)
                .help("CI green")
        case .failure, .error:
            Image(systemName: "xmark.circle.fill")
                .foregroundStyle(.red)
                .help("CI failing")
        case .pending, .expected:
            Image(systemName: "clock.badge.questionmark")
                .foregroundStyle(.orange)
                .help("CI running")
        }
    }
}
