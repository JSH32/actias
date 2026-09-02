import Document, { Head, Html, Main, NextScript } from 'next/document';

/**
 * A disposed monaco view can fire one final self-scheduled render (the
 * cursor blink queues repaints; disposal during a pane remount can land
 * inside that window). The error is monaco's own, surfaces via its
 * unexpected-error handler's deferred rethrow, and means nothing beyond
 * "that editor is gone", but the dev overlay treats any window error as
 * a crash.
 *
 * Suppression must be the first registered listener to work: later
 * listeners still run after preventDefault, so a component-level guard
 * can never shield the overlay. An inline document script runs before
 * the app bundle registers anything, and stopImmediatePropagation keeps
 * the event from every later listener. Scoped hard: monaco's file, the
 * dead-view signatures, nothing else.
 */
const MONACO_DISPOSAL_GUARD = `
window.addEventListener('error', function (event) {
  var fromMonaco = (event.filename || '').indexOf('monaco') !== -1;
  var deadView =
    (event.message || '').indexOf('domNode') !== -1 ||
    (event.message || '').indexOf('_glyphMarginWidgets') !== -1;
  if (fromMonaco && deadView) {
    event.stopImmediatePropagation();
    event.preventDefault();
  }
});
window.addEventListener('unhandledrejection', function (event) {
  var reason = event.reason || {};
  if (reason.name === 'Canceled' || reason.message === 'Canceled') {
    event.stopImmediatePropagation();
    event.preventDefault();
  }
});
`;

export default class _Document extends Document {
  render() {
    return (
      <Html>
        <Head>
          <script dangerouslySetInnerHTML={{ __html: MONACO_DISPOSAL_GUARD }} />
        </Head>
        <body>
          <Main />
          <NextScript />
        </body>
      </Html>
    );
  }
}
