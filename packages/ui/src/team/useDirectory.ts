import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { readPolicies, type ReadPolicy } from "./access-model";
import { describeError, isForbidden } from "./errors";
import type { Account, Person, PolicyManagement, PolicyRulesRead, PolicyTransport, Team, TeamTransport } from "./types";

export type Directory = {
  /** Active human members of the project's organization (administrators only). */
  members: Person[];
  /** Instance accounts, only for instance administrators. */
  accounts: Account[];
  teams: Team[];
  management: PolicyManagement | null;
  rules: PolicyRulesRead | null;
  policies: ReadPolicy[] | null;
  /** The server answered the administration reads for this project. */
  canAdminister: boolean;
  /** Members, teams and invitations can change (managed policy root). */
  canManage: boolean;
  loading: boolean;
  error: string;
  rulesError: string;
  /** Readable names for every principal seen so far. */
  people: Map<string, Person>;
  reload: () => Promise<void>;
};

/**
 * Loads who can be named in this project. The capability hint only avoids
 * asking for reads that would be refused; the server's answers decide what is
 * shown. Results are dropped when the scope (account, origin, project) changes.
 */
export function useDirectory({
  scope,
  administration,
  policy,
  instanceAdmin,
  self,
  administer,
}: {
  scope: string;
  administration: TeamTransport;
  policy?: PolicyTransport;
  instanceAdmin: boolean;
  self?: Person | null;
  /** Capability hint `managePolicy`; null when unknown. */
  administer: boolean | null;
}): Directory {
  const [members, setMembers] = useState<Person[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [teams, setTeams] = useState<Team[]>([]);
  const [management, setManagement] = useState<PolicyManagement | null>(null);
  const [rules, setRules] = useState<PolicyRulesRead | null>(null);
  const [canAdminister, setCanAdminister] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [rulesError, setRulesError] = useState("");
  const generation = useRef(0);
  const inputs = useRef({ administration, policy, instanceAdmin, administer });
  inputs.current = { administration, policy, instanceAdmin, administer };

  const reload = useCallback(async () => {
    const current = ++generation.current;
    const live = () => current === generation.current;
    const { administration, policy, instanceAdmin, administer } = inputs.current;
    setLoading(true);
    setError("");
    setRulesError("");
    try {
      const list = await administration<{ teams: Team[]; policyManagement: PolicyManagement }>({ action: "listTeams" });
      if (!live()) return;
      setTeams([...list.teams].sort((a, b) => a.name.localeCompare(b.name, "fr")));
      setManagement(list.policyManagement);
      let admin = false;
      if (administer !== false) {
        try {
          const eligible = await administration<{ members: Person[] }>({ action: "eligibleTeamMembers" });
          if (!live()) return;
          setMembers(eligible.members);
          admin = true;
        } catch (cause) {
          if (!live()) return;
          setMembers([]);
          if (!isForbidden(cause)) setError(describeError(cause, "La liste des membres n’a pas pu être lue."));
        }
      } else setMembers([]);
      setCanAdminister(admin);
      if (instanceAdmin) {
        try {
          const list = await administration<{ accounts: Account[] }>({ action: "listAccounts" });
          if (live()) setAccounts(list.accounts);
        } catch {
          if (live()) setAccounts([]);
        }
      }
      if (policy && admin) {
        try {
          const read = await policy<PolicyRulesRead>({ action: "readPolicyRules" });
          if (live()) setRules(read);
        } catch (cause) {
          if (!live()) return;
          setRules(null);
          setRulesError(describeError(cause, "Les règles d’accès n’ont pas pu être lues."));
        }
      } else setRules(null);
    } catch (cause) {
      if (!live()) return;
      setTeams([]);
      setManagement(null);
      setMembers([]);
      setRules(null);
      setCanAdminister(false);
      setError(describeError(cause, "Les informations de l’équipe n’ont pas pu être lues."));
    } finally {
      if (live()) setLoading(false);
    }
  }, []);

  useEffect(() => {
    setMembers([]);
    setAccounts([]);
    setTeams([]);
    setManagement(null);
    setRules(null);
    setCanAdminister(false);
    void reload();
    return () => {
      generation.current++;
    };
    // `scope` names account, origin and project; the hint may arrive later.
  }, [scope, administer, reload]);

  const policies = useMemo(() => (rules ? readPolicies(rules, teams) : null), [rules, teams]);
  const people = useMemo(() => {
    const map = new Map<string, Person>();
    for (const account of accounts) map.set(account.principalId, account);
    for (const member of members) map.set(member.principalId, member);
    if (self) map.set(self.principalId, { ...self, displayName: self.displayName || "Vous" });
    return map;
  }, [accounts, members, self]);
  const canManage = canAdminister && management?.mode === "managed";

  return { members, accounts, teams, management, rules, policies, canAdminister, canManage, loading, error, rulesError, people, reload };
}
