export function LensSurfaceHeader({
  mediaType,
  summary,
  title
}: {
  mediaType?: string;
  summary?: string;
  title: string;
}) {
  return (
    <header className="layrs-lens-surface__header">
      <strong>{title}</strong>
      {summary ?? mediaType ? <span>{summary ?? mediaType}</span> : null}
    </header>
  );
}
