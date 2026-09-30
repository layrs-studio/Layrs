// Vite resolves imported assets to their public URL.
declare module "*.svg" {
  const url: string;
  export default url;
}
