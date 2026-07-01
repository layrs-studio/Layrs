import type { Layer } from "@layrs/client-sdk";

export function LayerSelector({
  layers,
  selectedLayerId,
  onSelect
}: {
  layers: Layer[];
  selectedLayerId?: string;
  onSelect: (layerId: string) => void;
}) {
  if (layers.length === 0) {
    return (
      <div className="studio-layer-selector is-empty">
        <span>Layer</span>
        <strong>No Layers</strong>
      </div>
    );
  }

  const selectedLayer = layers.find((layer) => layer.id === selectedLayerId) ?? layers[0];

  return (
    <label className="studio-layer-selector">
      <span>Layer</span>
      <select onChange={(event) => onSelect(event.currentTarget.value)} value={selectedLayer.id}>
        {layers.map((layer) => (
          <option key={layer.id} value={layer.id}>
            {layer.name}
          </option>
        ))}
      </select>
    </label>
  );
}
