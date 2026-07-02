import type {
  Account,
  AuditEvent,
  Artifact,
  Device,
  Gate,
  Invitation,
  Layer,
  LayerAccessRegistry,
  Policy,
  Proof,
  Session,
  Space,
  Step,
  Team,
  TeamMember,
  TimelineItem,
  Weave,
  WeaveEvent,
  Workspace,
  WorkspaceMember
} from "./types";

export const account: Account = {
  id: "account-alexa",
  email: "alex@layrs.local",
  name: "Alex Martin",
  role: "owner",
  avatarInitials: "AM",
  createdAt: "2026-06-29T13:10:00Z"
};

export const workspace: Workspace = {
  id: "workspace-layrs-labs",
  name: "Layrs Labs",
  slug: "layrs-labs",
  description: "Local-first workspace for product, code and proof-driven delivery.",
  health: "needs-proof",
  updatedAt: "2026-06-29T16:10:00Z"
};

export const session: Session = {
  id: "session-studio-web",
  accountId: account.id,
  activeWorkspaceId: workspace.id,
  expiresAt: "2026-06-30T16:10:00Z",
  createdAt: "2026-06-29T16:10:00Z"
};

export const teams: Team[] = [
  {
    id: "team-studio",
    workspaceId: workspace.id,
    name: "Studio",
    purpose: "Owns the operator experience across web and desktop.",
    members: 5,
    gateResponsibility: "UX and release readiness"
  },
  {
    id: "team-core",
    workspaceId: workspace.id,
    name: "Core",
    purpose: "Owns store, graph, policies and gates.",
    members: 4,
    gateResponsibility: "Durability and invariants"
  },
  {
    id: "team-automation",
    workspaceId: workspace.id,
    name: "Automation",
    purpose: "Owns Steps, Weaves and generated Artifacts.",
    members: 3,
    gateResponsibility: "Proof completeness"
  }
];

export const workspaceMembers: WorkspaceMember[] = [
  {
    id: "membership-alexa",
    workspaceId: workspace.id,
    accountId: account.id,
    email: account.email,
    name: account.name,
    role: "owner",
    teamIds: ["team-studio", "team-core"],
    createdAt: "2026-06-29T13:12:00Z",
    updatedAt: "2026-06-29T16:10:00Z"
  },
  {
    id: "membership-core-reviewer",
    workspaceId: workspace.id,
    accountId: "account-core-reviewer",
    email: "core.reviewer@layrs.local",
    name: "Core Reviewer",
    role: "admin",
    teamIds: ["team-core"],
    createdAt: "2026-06-29T13:24:00Z",
    updatedAt: "2026-06-29T15:55:00Z"
  },
  {
    id: "membership-automation",
    workspaceId: workspace.id,
    accountId: "account-automation",
    email: "automation@layrs.local",
    name: "Automation Step",
    role: "member",
    teamIds: ["team-automation"],
    createdAt: "2026-06-29T13:31:00Z",
    updatedAt: "2026-06-29T16:05:00Z"
  }
];

export const teamMembers: TeamMember[] = workspaceMembers.flatMap((member) =>
  member.teamIds.map((teamId) => ({
    id: `team-membership-${teamId}-${member.accountId}`,
    workspaceId: member.workspaceId,
    teamId,
    accountId: member.accountId,
    email: member.email,
    name: member.name,
    role: member.role === "owner" || member.role === "admin" ? "maintainer" : "member",
    createdAt: member.createdAt
  }))
);

export const invitations: Invitation[] = [
  {
    id: "invitation-design-review",
    workspaceId: workspace.id,
    email: "design.review@layrs.local",
    role: "member",
    teamIds: ["team-studio"],
    status: "pending",
    createdAt: "2026-06-29T16:02:00Z",
    expiresAt: "2026-07-06T16:02:00Z"
  }
];

export const spaces: Space[] = [
  {
    id: "space-studio",
    workspaceId: workspace.id,
    teamId: "team-studio",
    name: "Studio Experience",
    description: "Web and desktop shell for navigating Spaces, Layers, Weaves and Proofs.",
    status: "needs-proof",
    currentLayerId: "layer-studio-review",
    updatedAt: "2026-06-29T16:08:00Z"
  },
  {
    id: "space-policy",
    workspaceId: workspace.id,
    teamId: "team-core",
    name: "Policy Runtime",
    description: "Declarative controls that resolve Gates without renaming product concepts.",
    status: "passing",
    currentLayerId: "layer-policy-base",
    updatedAt: "2026-06-29T15:52:00Z"
  },
  {
    id: "space-weave",
    workspaceId: workspace.id,
    teamId: "team-automation",
    name: "Weave Engine",
    description: "Narrative thread model that keeps decisions, Artifacts and Proofs connected.",
    status: "pending",
    currentLayerId: "layer-weave-experiment",
    updatedAt: "2026-06-29T15:37:00Z"
  }
];

export const layers: Layer[] = [
  {
    id: "layer-studio-base",
    spaceId: "space-studio",
    name: "Base shell",
    kind: "base",
    status: "stable",
    summary: "Stable application frame, navigation model and terminology.",
    artifactIds: ["artifact-shell-map"],
    stepIds: ["step-shell-audit"],
    gateIds: ["gate-studio-proof"]
  },
  {
    id: "layer-studio-review",
    spaceId: "space-studio",
    parentId: "layer-studio-base",
    name: "Review workspace",
    kind: "proposal",
    status: "review",
    summary: "Adds Weave review, Artifact browser, Timeline and policy surfaces.",
    artifactIds: ["artifact-weave-sample", "artifact-policy-draft"],
    stepIds: ["step-visual-pass"],
    gateIds: ["gate-studio-proof", "gate-policy-review"]
  },
  {
    id: "layer-policy-base",
    spaceId: "space-policy",
    name: "Policy matrix",
    kind: "base",
    status: "active",
    summary: "Initial rules for ownership, Step permissions and promotion Gates.",
    artifactIds: ["artifact-policy-draft"],
    stepIds: ["step-policy-check"],
    gateIds: ["gate-policy-review"]
  },
  {
    id: "layer-weave-experiment",
    spaceId: "space-weave",
    name: "Large diff reader",
    kind: "experiment",
    status: "active",
    summary: "Virtualized Weave stream that stays responsive with many events.",
    artifactIds: ["artifact-weave-sample"],
    stepIds: ["step-weave-load"],
    gateIds: ["gate-weave-performance"]
  }
];

export const artifacts: Artifact[] = [
  {
    id: "artifact-shell-map",
    spaceId: "space-studio",
    layerId: "layer-studio-base",
    name: "Studio surface map",
    type: "note",
    summary: "Dashboard, Spaces list, Layer tree, Weave review, Timeline and Proof panels.",
    location: "docs/studio/surface-map.md",
    updatedAt: "2026-06-29T15:58:00Z",
    sizeLabel: "4.2 KB",
    proofIds: ["proof-ux-review"],
    access: {
      mode: "write",
      canOpen: true,
      isRedacted: false
    }
  },
  {
    id: "artifact-weave-sample",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Large Weave sample",
    type: "step-output",
    summary: "Synthetic event stream for testing virtualized review rendering.",
    location: "fixtures/weaves/large-review.json",
    updatedAt: "2026-06-29T16:00:00Z",
    sizeLabel: "128 KB",
    proofIds: ["proof-load-model"],
    access: {
      mode: "none",
      canOpen: false,
      isRedacted: true,
      reason: "Restricted by Layer access policy"
    }
  },
  {
    id: "artifact-policy-draft",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Promotion policy draft",
    type: "report",
    summary: "Matrix of workspace, team and space rules for promotion and automated Steps.",
    location: "policies/promotion-policy.layrs.json",
    updatedAt: "2026-06-29T16:04:00Z",
    sizeLabel: "9.8 KB",
    proofIds: ["proof-policy-review"],
    access: {
      mode: "admin",
      canOpen: true,
      isRedacted: false
    }
  }
];

export const steps: Step[] = [
  {
    id: "step-shell-audit",
    spaceId: "space-studio",
    layerId: "layer-studio-base",
    name: "Check terminology alignment",
    status: "passing",
    actor: "human",
    startedAt: "2026-06-29T15:44:00Z",
    completedAt: "2026-06-29T15:52:00Z",
    artifactIds: ["artifact-shell-map"],
    proofIds: ["proof-ux-review"]
  },
  {
    id: "step-visual-pass",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    name: "Review app-first layout",
    status: "needs-proof",
    actor: "human",
    startedAt: "2026-06-29T16:03:00Z",
    artifactIds: ["artifact-shell-map"],
    proofIds: []
  },
  {
    id: "step-policy-check",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Evaluate promotion policy",
    status: "passing",
    actor: "automation",
    startedAt: "2026-06-29T15:46:00Z",
    completedAt: "2026-06-29T15:49:00Z",
    artifactIds: ["artifact-policy-draft"],
    proofIds: ["proof-policy-review"]
  },
  {
    id: "step-weave-load",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Generate large Weave review sample",
    status: "pending",
    actor: "automation",
    startedAt: "2026-06-29T16:05:00Z",
    artifactIds: ["artifact-weave-sample"],
    proofIds: ["proof-load-model"]
  }
];

export const gates: Gate[] = [
  {
    id: "gate-studio-proof",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    name: "Studio proof required",
    status: "needs-proof",
    ownerTeamId: "team-studio",
    requiredProofKinds: ["review", "snapshot"],
    summary: "Promotion needs visible proof that web and desktop shells expose the same concepts."
  },
  {
    id: "gate-policy-review",
    spaceId: "space-policy",
    layerId: "layer-policy-base",
    name: "Policy owner review",
    status: "passing",
    ownerTeamId: "team-core",
    requiredProofKinds: ["review"],
    summary: "Policy changes have a Core Team review and clear scope."
  },
  {
    id: "gate-weave-performance",
    spaceId: "space-weave",
    layerId: "layer-weave-experiment",
    name: "Large Weave performance",
    status: "pending",
    ownerTeamId: "team-automation",
    requiredProofKinds: ["test"],
    summary: "Large review streams must render with virtualization before promotion."
  }
];

export const proofs: Proof[] = [
  {
    id: "proof-ux-review",
    targetId: "artifact-shell-map",
    targetType: "artifact",
    kind: "review",
    status: "accepted",
    title: "Terminology review",
    evidence: "Workspace, Team and Space are used as primary labels across the shell.",
    createdAt: "2026-06-29T15:53:00Z",
    gateId: "gate-studio-proof",
    artifactId: "artifact-shell-map"
  },
  {
    id: "proof-policy-review",
    targetId: "gate-policy-review",
    targetType: "gate",
    kind: "review",
    status: "accepted",
    title: "Policy owner approval",
    evidence: "Core Team owns promotion policy edits for this Space.",
    createdAt: "2026-06-29T15:55:00Z",
    gateId: "gate-policy-review",
    artifactId: "artifact-policy-draft"
  },
  {
    id: "proof-load-model",
    targetId: "artifact-weave-sample",
    targetType: "artifact",
    kind: "test",
    status: "reviewing",
    title: "Virtualized Weave model",
    evidence: "Large event streams are windowed by row height and scroll offset.",
    createdAt: "2026-06-29T16:06:00Z",
    gateId: "gate-weave-performance",
    artifactId: "artifact-weave-sample"
  },
  {
    id: "proof-desktop-snapshot",
    targetId: "layer-studio-review",
    targetType: "layer",
    kind: "snapshot",
    status: "missing",
    title: "Desktop shell snapshot",
    evidence: "Pending local Tauri dependency install.",
    createdAt: "2026-06-29T16:09:00Z",
    gateId: "gate-studio-proof"
  }
];

export const policies: Policy[] = [
  {
    id: "policy-workspace-promotion",
    scope: "workspace",
    targetId: workspace.id,
    name: "Workspace promotion",
    appliesTo: "Layer promotion",
    effect: "require-proof",
    rules: [
      {
        id: "rule-promotion-review",
        subject: "layer",
        action: "promote",
        effect: "require-proof",
        gateId: "gate-studio-proof"
      },
      {
        id: "rule-policy-owner",
        subject: "space",
        action: "edit-policy",
        effect: "require-proof",
        gateId: "gate-policy-review"
      }
    ]
  },
  {
    id: "policy-automation-scope",
    scope: "team",
    targetId: "team-automation",
    name: "Automation scope",
    appliesTo: "Automated Steps",
    effect: "allow",
    rules: [
      {
        id: "rule-step-artifacts",
        subject: "step",
        action: "create-artifact",
        effect: "allow"
      },
      {
        id: "rule-step-approval",
        subject: "step",
        action: "approve-gate",
        effect: "block"
      }
    ]
  },
  {
    id: "policy-space-performance",
    scope: "space",
    targetId: "space-weave",
    name: "Weave performance",
    appliesTo: "Weave review",
    effect: "require-proof",
    rules: [
      {
        id: "rule-weave-load",
        subject: "space",
        action: "render-large-weave",
        effect: "require-proof",
        gateId: "gate-weave-performance"
      }
    ]
  }
];

function createWeaveEvents(count: number): WeaveEvent[] {
  const kinds: WeaveEvent["kind"][] = ["intent", "change", "decision", "comment", "proof", "artifact"];

  return Array.from({ length: count }, (_, index) => {
    const kind = kinds[index % kinds.length];
    const eventNumber = index + 1;

    return {
      id: `weave-event-${eventNumber.toString().padStart(3, "0")}`,
      weaveId: "weave-studio-review",
      kind,
      title: `${kind} ${eventNumber}`,
      actor: index % 3 === 0 ? "Studio Team" : index % 3 === 1 ? "Automation Step" : "Core Reviewer",
      at: new Date(Date.UTC(2026, 5, 29, 14, Math.floor(index / 2), (index % 2) * 30)).toISOString(),
      summary:
        index % 5 === 0
          ? "Large diff checkpoint with related Artifact and Proof references kept in the review stream."
          : "Incremental context update for the selected Layer and its promotion path.",
      diffStats:
        kind === "change"
          ? {
              files: 2 + (index % 9),
              additions: 12 + index * 3,
              removals: 4 + index
            }
          : undefined,
      artifactIds: index % 4 === 0 ? ["artifact-weave-sample"] : [],
      proofIds: index % 6 === 0 ? ["proof-load-model"] : []
    };
  });
}

export const weaves: Weave[] = [
  {
    id: "weave-studio-review",
    spaceId: "space-studio",
    layerId: "layer-studio-review",
    title: "Studio shell review",
    status: "needs-proof",
    events: createWeaveEvents(180)
  }
];

export const timeline: TimelineItem[] = [
  {
    id: "timeline-terminology",
    at: "2026-06-29T15:44:00Z",
    title: "Terminology aligned",
    summary: "Studio shell labels were mapped to Workspace, Team, Space, Layer and Artifact.",
    status: "passing",
    relatedIds: ["artifact-shell-map", "proof-ux-review"]
  },
  {
    id: "timeline-policy",
    at: "2026-06-29T15:55:00Z",
    title: "Policy reviewed",
    summary: "Promotion rules require Proof before a Layer can pass its Gate.",
    status: "passing",
    relatedIds: ["policy-workspace-promotion", "gate-policy-review"]
  },
  {
    id: "timeline-weave",
    at: "2026-06-29T16:06:00Z",
    title: "Weave load model queued",
    summary: "Large review data is available for virtualization validation.",
    status: "pending",
    relatedIds: ["weave-studio-review", "gate-weave-performance"]
  },
  {
    id: "timeline-desktop",
    at: "2026-06-29T16:09:00Z",
    title: "Desktop snapshot pending",
    summary: "Tauri shell is scaffolded; dependency install and runtime proof remain open.",
    status: "needs-proof",
    relatedIds: ["proof-desktop-snapshot", "gate-studio-proof"]
  }
];

export const accessRegistries: LayerAccessRegistry[] = [
  {
    id: "access-layer-studio-base",
    workspaceId: workspace.id,
    layerId: "layer-studio-base",
    updatedAt: "2026-06-29T16:04:00Z",
    rules: [
      {
        id: "access-layer-studio-base-account",
        subjectKind: "account",
        subjectId: account.id,
        subjectName: account.name,
        mode: "admin"
      },
      {
        id: "access-layer-studio-base-team",
        subjectKind: "team",
        subjectId: "team-studio",
        subjectName: "Studio",
        mode: "write"
      }
    ]
  },
  {
    id: "access-layer-studio-review",
    workspaceId: workspace.id,
    layerId: "layer-studio-review",
    updatedAt: "2026-06-29T16:05:00Z",
    rules: [
      {
        id: "access-layer-studio-review-account",
        subjectKind: "account",
        subjectId: account.id,
        subjectName: account.name,
        mode: "admin"
      },
      {
        id: "access-layer-studio-review-core",
        subjectKind: "team",
        subjectId: "team-core",
        subjectName: "Core",
        mode: "read"
      },
      {
        id: "access-layer-studio-review-automation",
        subjectKind: "team",
        subjectId: "team-automation",
        subjectName: "Automation",
        mode: "none"
      }
    ]
  },
  {
    id: "access-layer-policy-base",
    workspaceId: workspace.id,
    layerId: "layer-policy-base",
    updatedAt: "2026-06-29T16:05:00Z",
    rules: [
      {
        id: "access-layer-policy-base-core",
        subjectKind: "team",
        subjectId: "team-core",
        subjectName: "Core",
        mode: "admin"
      },
      {
        id: "access-layer-policy-base-studio",
        subjectKind: "team",
        subjectId: "team-studio",
        subjectName: "Studio",
        mode: "read"
      }
    ]
  }
];

export const devices: Device[] = [
  {
    id: "device-web-chrome",
    accountId: account.id,
    name: "Chrome on Windows",
    kind: "browser",
    status: "trusted",
    lastSeenAt: "2026-06-29T16:14:00Z"
  },
  {
    id: "device-desktop-preview",
    accountId: account.id,
    name: "Layrs Desktop Preview",
    kind: "desktop",
    status: "pending",
    lastSeenAt: "2026-06-29T15:44:00Z"
  }
];

export const auditEvents: AuditEvent[] = [
  {
    id: "audit-login",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "session.login",
    target: "studio-web",
    summary: "Signed in to Studio Web.",
    at: "2026-06-29T16:10:00Z"
  },
  {
    id: "audit-access",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "layer.access.reviewed",
    target: "layer-studio-review",
    summary: "Reviewed access registry for the Studio review layer.",
    at: "2026-06-29T16:12:00Z"
  },
  {
    id: "audit-redaction",
    workspaceId: workspace.id,
    actorAccountId: account.id,
    action: "artifact.redacted",
    target: "artifact-weave-sample",
    summary: "Artifact contents hidden by Layer access policy.",
    at: "2026-06-29T16:13:00Z"
  }
];

