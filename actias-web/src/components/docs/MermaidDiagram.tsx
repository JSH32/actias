import * as React from 'react';
import classes from './MermaidDiagram.module.css';

/**
 * Renders a ```mermaid fence client-side. The library is dynamically
 * imported so only docs pages that actually carry a diagram pay for it;
 * until the svg arrives (or if the source fails to parse) the raw text
 * stays visible, so the page never loses information.
 */

/** Theme variables mapped onto the site's tokens (tokens.css). */
const THEME = {
  darkMode: true,
  background: '#12151d',
  fontFamily: "'Sora', system-ui, sans-serif",
  fontSize: '14px',
  primaryColor: '#1a1e29',
  primaryTextColor: '#e8ebf0',
  primaryBorderColor: '#7c8699',
  secondaryColor: '#12151d',
  tertiaryColor: '#12151d',
  lineColor: '#7c8699',
  textColor: '#e8ebf0',
  clusterBkg: 'rgba(255, 255, 255, 0.02)',
  clusterBorder: '#3a4152',
  edgeLabelBackground: '#12151d',
  actorBkg: '#1a1e29',
  actorBorder: '#7c8699',
  actorTextColor: '#e8ebf0',
  actorLineColor: '#3a4152',
  signalColor: '#9aa3b2',
  signalTextColor: '#e8ebf0',
  labelBoxBkgColor: '#1a1e29',
  labelBoxBorderColor: '#3a4152',
  labelTextColor: '#e8ebf0',
  loopTextColor: '#9aa3b2',
  noteBkgColor: '#1a1e29',
  noteTextColor: '#e8ebf0',
  noteBorderColor: 'rgba(163, 230, 180, 0.4)',
  activationBkgColor: 'rgba(163, 230, 180, 0.12)',
  activationBorderColor: 'rgba(163, 230, 180, 0.4)',
};

let initialized = false;
let sequence = 0;

export function MermaidDiagram({ chart }: { chart: string }) {
  const [svg, setSvg] = React.useState<string | null>(null);

  React.useEffect(() => {
    let live = true;
    (async () => {
      try {
        const mermaid = (await import('mermaid')).default;
        if (!initialized) {
          mermaid.initialize({
            startOnLoad: false,
            theme: 'base',
            themeVariables: THEME,
          });
          initialized = true;
        }
        sequence += 1;
        const rendered = await mermaid.render(
          `docs-mermaid-${sequence}`,
          chart.trim(),
        );
        if (live) setSvg(rendered.svg);
      } catch {
        // A parse failure keeps the source text on screen instead.
      }
    })();
    return () => {
      live = false;
    };
  }, [chart]);

  if (svg === null) {
    return <pre className={classes.pending}>{chart.trim()}</pre>;
  }
  return (
    <div
      className={classes.diagram}
      // Mermaid's own output, from a fence in our checked-in docs; no
      // user-provided content ever reaches this component.
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
