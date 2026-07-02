import { legacyRegistryToLayerAccessPolicy } from "./access";
import {
  accessRegistries,
  account,
  artifacts,
  auditEvents,
  devices,
  gates,
  invitations,
  layers,
  policies,
  proofs,
  session,
  spaces,
  steps,
  teamMembers,
  teams,
  timeline,
  weaves,
  workspace,
  workspaceMembers
} from "./fixture-data";
import type { LayerAccessPolicy, StudioFixture } from "./types";

const layerAccessPolicies: LayerAccessPolicy[] = accessRegistries.map(legacyRegistryToLayerAccessPolicy);

const fixture: StudioFixture = {
  account,
  session,
  workspace,
  workspaces: [workspace],
  teams,
  spaces,
  layers,
  artifacts,
  steps,
  weaves,
  proofs,
  gates,
  policies,
  timeline,
  layerAccessPolicies,
  accessRegistries,
  workspaceMembers,
  teamMembers,
  invitations,
  devices,
  auditEvents
};

export function getStudioFixture(): StudioFixture {
  return fixture;
}

export const studioFixture = fixture;

