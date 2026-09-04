import * as React from 'react';
import { Maximize2, X } from 'lucide-react';
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

  const [enlarged, setEnlarged] = React.useState(false);

  if (svg === null) {
    return <pre className={classes.pending}>{chart.trim()}</pre>;
  }
  return (
    <div className={classes.frame}>
      <div
        className={classes.diagram}
        // Mermaid's own output, from a fence in our checked-in docs; no
        // user-provided content ever reaches this component.
        dangerouslySetInnerHTML={{ __html: svg }}
      />
      <button
        type="button"
        className={classes.enlarge}
        onClick={() => setEnlarged(true)}
        aria-label="Enlarge diagram"
        title="Enlarge: scroll to zoom, drag to pan"
      >
        <Maximize2 size={13} strokeWidth={1.7} />
      </button>
      {enlarged && <Viewer svg={svg} onClose={() => setEnlarged(false)} />}
    </div>
  );
}

/**
 * The diagram full-window: the wheel zooms about the pointer, a drag
 * pans, a double click resets, Escape or the close control leaves.
 * Transforms are applied to the svg's wrapper, so the svg itself is
 * the one mermaid rendered.
 */
function Viewer({ svg, onClose }: { svg: string; onClose: () => void }) {
  const [view, setView] = React.useState({ scale: 1, x: 0, y: 0 });
  const drag = React.useRef<{
    x: number;
    y: number;
    ox: number;
    oy: number;
  } | null>(null);

  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    const overflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      window.removeEventListener('keydown', onKey);
      document.body.style.overflow = overflow;
    };
  }, [onClose]);

  // The wheel listener is attached natively: React registers wheel
  // listeners as passive, and a passive listener cannot stop the page
  // from scrolling under the diagram. The pointer's position is read
  // before the state updater runs, which is after the event is over.
  const stage = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    const element = stage.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const factor = Math.exp(-event.deltaY * 0.0015);
      const rect = element.getBoundingClientRect();
      const px = event.clientX - rect.left - rect.width / 2;
      const py = event.clientY - rect.top - rect.height / 2;
      setView((current) => {
        const scale = Math.min(8, Math.max(0.2, current.scale * factor));
        // Zoom about the pointer: keep the point under it fixed.
        const ratio = scale / current.scale;
        return {
          scale,
          x: px - (px - current.x) * ratio,
          y: py - (py - current.y) * ratio,
        };
      });
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, []);
  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    drag.current = {
      x: event.clientX,
      y: event.clientY,
      ox: view.x,
      oy: view.y,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const from = drag.current;
    if (!from) return;
    setView((current) => ({
      ...current,
      x: from.ox + (event.clientX - from.x),
      y: from.oy + (event.clientY - from.y),
    }));
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  return (
    <div className={classes.viewer} role="dialog" aria-label="Diagram">
      <div className={classes.viewerBar}>
        <span>scroll to zoom, drag to pan, double click to reset</span>
        <span>{Math.round(view.scale * 100)}%</span>
        <button
          type="button"
          className={classes.viewerClose}
          onClick={onClose}
          aria-label="Close"
        >
          <X size={15} strokeWidth={1.7} />
        </button>
      </div>
      <div
        ref={stage}
        className={classes.viewerStage}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onDoubleClick={() => setView({ scale: 1, x: 0, y: 0 })}
      >
        <div
          className={classes.viewerSvg}
          style={{
            transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
          }}
          // The same mermaid output as the inline diagram.
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      </div>
    </div>
  );
}
