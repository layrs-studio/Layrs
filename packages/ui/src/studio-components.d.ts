import type { ReactNode } from "react";
import type { Artifact, Gate, GateStatus, Policy, Proof, WeaveEvent } from "@layrs/client-sdk";
export interface SidebarItem {
    id: string;
    label: string;
    eyebrow?: string;
    isActive?: boolean;
    meta?: string;
}
export interface AppShellProps {
    productName: string;
    workspaceName: string;
    sidebar: ReactNode;
    toolbar?: ReactNode;
    children: ReactNode;
}
export declare function AppShell({ productName, workspaceName, sidebar, toolbar, children }: AppShellProps): import("react").JSX.Element;
export interface SidebarProps {
    items: SidebarItem[];
    footer?: ReactNode;
}
export declare function Sidebar({ items, footer }: SidebarProps): import("react").JSX.Element;
export interface StatusPillProps {
    status: GateStatus | Proof["status"];
    label?: string;
}
export declare function StatusPill({ status, label }: StatusPillProps): import("react").JSX.Element;
export interface VirtualWeaveListProps {
    events: WeaveEvent[];
    height?: number;
    rowHeight?: number;
}
export declare function VirtualWeaveList({ events, height, rowHeight }: VirtualWeaveListProps): import("react").JSX.Element;
export interface ArtifactCardProps {
    artifact: Artifact;
    proofs?: Proof[];
}
export declare function ArtifactCard({ artifact, proofs }: ArtifactCardProps): import("react").JSX.Element;
export interface ProofPanelProps {
    gates: Gate[];
    proofs: Proof[];
}
export declare function ProofPanel({ gates, proofs }: ProofPanelProps): import("react").JSX.Element;
export interface PolicyMatrixProps {
    policies: Policy[];
}
export declare function PolicyMatrix({ policies }: PolicyMatrixProps): import("react").JSX.Element;
//# sourceMappingURL=studio-components.d.ts.map