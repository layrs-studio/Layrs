import { expect, test } from "@playwright/test";
import {
  accessLabel,
  captureNames,
  decisionSummary,
  decisionSuperseded,
  explainCollabError,
  folderChildren,
  historyFilterRequest,
  localDayBoundary,
  namedCaptures,
  personInSentence,
  personLabel,
  projectReferences,
  searchFiles,
} from "../../packages/ui/src/collab/model";
import { exactReviewVersion } from "../../packages/ui/src/native-review-context";

// Pure presentation rules of the collaboration pages (P06–P14). Fictional data only.
const head = `blake3:${"a".repeat(64)}`;
const second = `blake3:${"b".repeat(64)}`;
const bootstrap = {
  layers: [
    { record: { layerId: "layer:main", name: "main" }, headStepIds: [head] },
    { record: { layerId: "layer:site", name: "Refonte accueil" }, headStepIds: [head, second] },
  ],
  markers: [{ record: { markerId: "marker:1", name: "avant-refonte", title: "", notes: "", targetStepId: second, createdAt: "2026-09-01T10:00:00Z" } }],
  releases: [{ record: { releaseId: "release:1", version: "1.2.0", title: "", notes: "", targetStepId: head, createdAt: "2026-09-02T10:00:00Z" } }],
  contentAccess: { mode: "restricted" },
  unknownField: { ignored: true },
};

const entry = (path: string, kind: "file" | "tree", contentReadable = true, writable = contentReadable) => ({
  path,
  target: kind === "file" ? ({ kind, fileId: head } as const) : ({ kind, treeId: head } as const),
  executable: false,
  contentReadable,
  writable,
});

test("references name captures by Layer, version and marker", () => {
  const references = projectReferences(bootstrap);
  expect(references.restricted).toBe(true);
  expect(captureNames(head, references)).toEqual(["Dernière capture de main", "Dernière capture de Refonte accueil", "Version 1.2.0"]);
  expect(captureNames(second, references)).toEqual(["Dernière capture de Refonte accueil", "Repère avant-refonte"]);
  const options = namedCaptures(references);
  expect(options.map((option) => option.label)).toContain("Refonte accueil · tête 2 sur 2");
  expect(options.filter((option) => option.group === "Versions")).toEqual([{ stepId: head, label: "Version 1.2.0", group: "Versions" }]);
  expect(projectReferences(null)).toEqual({ layers: [], markers: [], releases: [], restricted: false });
});

test("people are never invented", () => {
  const people = { currentPrincipalId: "principal:me", names: { "principal:me": "Alice" } };
  expect(personLabel("principal:me", people)).toBe("Vous");
  expect(personLabel("principal:other", people)).toBe("Autre compte");
  expect(personLabel(null, people)).toBe("Compte inconnu");
  expect(personInSentence("principal:other", { names: { "principal:other": "Bob" } })).toBe("Bob");
  expect(personInSentence("principal:x")).toBe("un autre compte");
});

test("decisions count only for the version they were made on", () => {
  expect(decisionSuperseded({ action: "approve", version: 1 }, 2)).toBe(true);
  expect(decisionSuperseded({ action: "request_changes", version: 2 }, 2)).toBe(false);
  expect(decisionSuperseded({ action: "comment", version: 1 }, 2)).toBe(false);
  expect(decisionSummary({ state: "pending", approvals: 1, requiredApprovals: 2, blockingDecisions: 0, stale: false })).toMatchObject({ label: "En attente d’approbation", detail: "1/2 approbations" });
  expect(decisionSummary({ state: "approved", approvals: 2, requiredApprovals: 2, blockingDecisions: 0, stale: true })?.label).toBe("Version à actualiser");
  expect(decisionSummary({ state: "changes_requested", approvals: 0, requiredApprovals: 1, blockingDecisions: 1, stale: false })?.tone).toBe("danger");
  expect(decisionSummary(undefined)).toBeNull();
});

test("a notification never falls back to another review version", () => {
  const versions = [{ reviewId: "review:x", version: 1 }, { reviewId: "review:x", version: 2 }];
  expect(exactReviewVersion(versions, { reviewId: "review:x", version: 1 })).toBe(versions[0]);
  expect(() => exactReviewVersion(versions, { reviewId: "review:x", version: 3 })).toThrow("version exacte");
});

test("folders are browsed within loaded entries and forbidden content stays opaque", () => {
  const entries = [entry("src", "tree"), entry("src/app.ts", "file"), entry("docs/guide.md", "file"), entry("secret.txt", "file", false), entry("README.md", "file", true, false)];
  const root = folderChildren(entries, "");
  expect(root.map((row) => `${row.kind}:${row.name}${row.implicit ? "*" : ""}`)).toEqual(["tree:docs*", "tree:src", "file:README.md", "file:secret.txt"]);
  expect(folderChildren(entries, "src").map((row) => row.path)).toEqual(["src/app.ts"]);
  expect(accessLabel(root.find((row) => row.name === "secret.txt")!)).toEqual({ label: "Contenu non accessible", tone: "warning" });
  expect(accessLabel(root.find((row) => row.name === "README.md")!).label).toBe("Lecture seule");
  expect(searchFiles(entries, "GUIDE").map((row) => row.path)).toEqual(["docs/guide.md"]);
  expect(searchFiles(entries, "  ")).toEqual([]);
});

test("history filters are sent as RFC 3339 instants of the local calendar", () => {
  const since = localDayBoundary("2026-09-01", false)!;
  const until = localDayBoundary("2026-09-30", true)!;
  expect(since).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  expect(new Date(since).getTime()).toBe(new Date(2026, 8, 1, 0, 0, 0).getTime());
  expect(new Date(until).getTime()).toBe(new Date(2026, 8, 30, 23, 59, 59).getTime());
  expect(localDayBoundary("01/09/2026", false)).toBeNull();
  expect(historyFilterRequest({ from: head, path: "", since: "", until: "", actor: " actor:x " })).toEqual({ actorId: "actor:x", since: null, until: null });
});

test("errors are explained in French and keep their technical code", () => {
  const gate = Object.assign(new Error("release_gate_rejected"), { status: 400, code: "release_gate_rejected" });
  expect(explainCollabError(gate)).toMatchObject({ code: "release_gate_rejected", forbidden: false });
  expect(explainCollabError(gate).message).toContain("Publication refusée");
  expect(explainCollabError(Object.assign(new Error("invalid_capability"), { status: 403, code: "invalid_capability" }))).toMatchObject({ forbidden: true });
  expect(explainCollabError("authorization_epoch_changed")).toMatchObject({ stale: true, code: "authorization_epoch_changed" });
  expect(explainCollabError(Object.assign(new Error("Erreur serveur 403"), { status: 403, code: null }))).toMatchObject({ forbidden: true, code: null });
  expect(explainCollabError(new Error("Réponse serveur absente."))).toEqual({ message: "Réponse serveur absente.", code: null, forbidden: false, stale: false });
});
