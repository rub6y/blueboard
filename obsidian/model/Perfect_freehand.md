# Perfect freehand

Perfect freehand is an open-source freehand-drawing library by Steve Ruiz, used by tldraw
and Excalidraw for their pen tools.

Perfect freehand's "streamline" parameter inspired [[App_crate]]'s pointer-smoothing fix:
pulling each incoming raw point toward a running average of recent positions (an
exponential moving average) before the point is decimated or handed to [[Fitting]].

Perfect freehand's full approach builds a variable-width outline polygon rather than a
centerline-plus-width stroke — a bigger architectural difference from [[Blueboard]]'s
approach, not adopted; only the streamline idea was.
