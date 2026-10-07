import type { Core } from "cytoscape";

/** Canvas colors follow the same tokens as the surrounding shadcn surfaces. */
export function observeGraphTheme(cy: Core, mesh = false): () => void {
  const apply = () => {
    const css = getComputedStyle(document.documentElement);
    const token = (name: string) => css.getPropertyValue(name).trim();
    const surface = token("--card"),
      text = token("--foreground"),
      muted = token("--muted-foreground"),
      border = token("--input"),
      primary = token("--primary");
    const styles = cy.style();
    styles.selector(mesh ? "node.mesh-node" : "node").style({
      "background-color": surface,
      color: text,
    });
    styles.selector("edge").style({
      "line-color": border,
      "target-arrow-color": border,
      color: muted,
      "text-background-color": surface,
    });
    if (mesh) {
      styles.selector("node.mesh-group").style({
        "background-color": surface,
        "border-color": border,
      });
      for (const region of ["workspaces", "execution", "registry"])
        styles
          .selector(`node.region-${region}`)
          .style({ "background-color": surface, "border-color": border });
    } else {
      styles.selector("node.root").style({ "background-color": primary });
      styles.selector("node:selected").style({ "border-color": primary });
    }
    styles.update();
  };
  apply();
  const observer = new MutationObserver(apply);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  return () => observer.disconnect();
}
