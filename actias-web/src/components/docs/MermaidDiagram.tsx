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
  background: '#0f1211',
  fontFamily: "'Sora', system-ui, sans-serif",
  fontSize: '14px',
  primaryColor: '#171c1a',
  primaryTextColor: '#e9ede9',
  primaryBorderColor: '#7e887f',
  secondaryColor: '#0f1211',
  tertiaryColor: '#0f1211',
  lineColor: '#7e887f',
  textColor: '#e9ede9',
  clusterBkg: 'rgba(255, 255, 255, 0.02)',
  clusterBorder: '#2a322c',
  edgeLabelBackground: '#0f1211',
  actorBkg: '#171c1a',
  actorBorder: '#7e887f',
  actorTextColor: '#e9ede9',
  actorLineColor: '#2a322c',
  signalColor: '#97a09a',
  signalTextColor: '#e9ede9',
  labelBoxBkgColor: '#171c1a',
  labelBoxBorderColor: '#2a322c',
  labelTextColor: '#e9ede9',
  loopTextColor: '#97a09a',
  noteBkgColor: '#171c1a',
  noteTextColor: '#e9ede9',
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
