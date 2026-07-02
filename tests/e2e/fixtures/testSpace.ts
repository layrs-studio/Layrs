export interface TestSpaceFile {
  path: string;
  size?: number;
  state?: "added" | "modified" | "deleted";
  summary?: string;
  body?: string;
}

export interface TestSpace {
  name: string;
  rootPath: string;
  files: TestSpaceFile[];
}

export function makeTestSpace(overrides: Partial<TestSpace> = {}): TestSpace {
  const name = overrides.name ?? "Mirror Anti Loss";
  const rootPath = overrides.rootPath ?? "D:\\Layrs\\tmp\\mirror-anti-loss";

  return {
    name,
    rootPath,
    files: overrides.files ?? [
      {
        path: "README.md",
        size: 512,
        state: "modified",
        summary: "README.md has local edits that must be captured.",
        body: "A visible README change from the working tree."
      },
      {
        path: "src/new-surface.ts",
        size: 384,
        state: "added",
        summary: "src/new-surface.ts is newly added.",
        body: "export const surface = 'anti-loss';"
      }
    ]
  };
}
