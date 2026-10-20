import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import research from '../../../examples/graphs/deep-academic-research.json';
import { asDefinition } from './model';
import { layoutWorkflow } from './graph';
import { WorkflowCanvas } from './WorkflowCanvas';

describe('workflow first render', () => {
  it('renders cached geometry as ready immediately without waiting for a mount effect', async () => {
    const name = 'synchronous-canvas';
    const render = () => renderToStaticMarkup(<WorkflowCanvas graph={research} name={name} mode="edit" onPick={() => undefined} />);
    expect(render()).toContain('aria-busy="true"');
    await layoutWorkflow(asDefinition(research, name));
    const html = render();
    expect(html).toContain('aria-busy="false"');
    expect(html).not.toContain('正在整理布局');
    const changed = { ...research, edges: research.edges.slice(1) };
    const invalidated = renderToStaticMarkup(<WorkflowCanvas graph={changed} name={name} mode="edit" onPick={() => undefined} />);
    expect(invalidated).toContain('aria-busy="true"');
  });
});
