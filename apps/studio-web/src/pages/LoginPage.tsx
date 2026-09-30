import { useState, type FormEvent } from "react";
import { Banner, Brand, Button, PasswordField, TextField } from "@layrs/ui";
import { ApiError, useSession } from "../app/session";

/** P01: join this instance. The requested page opens after sign-in. */
export function LoginPage() {
  const { signIn } = useSession();
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; detail: string } | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || !login || !password) return;
    setBusy(true);
    setError(null);
    try {
      await signIn(login, password);
      setPassword("");
    } catch (cause) {
      if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403 || cause.status === 429)) {
        setError({
          title: cause.status === 429 ? "Trop de tentatives" : "Connexion refusée",
          detail:
            cause.status === 429
              ? "Patientez quelques minutes avant de réessayer."
              : "Identifiant ou mot de passe incorrect. Vérifiez-les ou contactez l’administrateur de votre instance.",
        });
      } else {
        setError({ title: "Connexion impossible", detail: cause instanceof Error ? cause.message : String(cause) });
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="login" id="layrs-main">
      <form className="login__card" onSubmit={(event) => void submit(event)} noValidate>
        <Brand size={40} />
        <div className="login__heading">
          <h1>Retrouvez votre équipe</h1>
          <p className="login__instance">
            Instance <strong>{location.host}</strong>
          </p>
        </div>
        {error && (
          <Banner tone="danger" title={error.title}>
            {error.detail}
          </Banner>
        )}
        <TextField label="Identifiant" name="username" autoComplete="username" value={login} disabled={busy} autoFocus onChange={(event) => setLogin(event.target.value)} />
        <PasswordField label="Mot de passe" name="password" autoComplete="current-password" value={password} disabled={busy} onChange={(event) => setPassword(event.target.value)} />
        <Button
          type="submit"
          variant="primary"
          size="lg"
          busy={busy}
          busyLabel="Connexion…"
          disabledReason={!login || !password ? "Saisissez votre identifiant et votre mot de passe." : undefined}
        >
          Se connecter
        </Button>
        <p className="login__help">Pas encore de compte ? Demandez-le à l’administrateur de votre instance Layrs.</p>
      </form>
    </main>
  );
}
