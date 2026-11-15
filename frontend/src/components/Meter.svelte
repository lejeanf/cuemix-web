<script lang="ts">
  import { meterBank } from '../lib/meters';

  let { index, horizontal = false, label = 'Level meter' }: { index: number; horizontal?: boolean; label?: string } = $props();
  let canvas: HTMLCanvasElement;

  $effect(() => {
    return meterBank.register(canvas, index, horizontal);
  });
</script>

<canvas bind:this={canvas} class:horizontal aria-hidden="true" data-label={label}></canvas>

<style>
  canvas {
    width: 8px;
    height: 100%;
    min-height: 60px;
    border-radius: 2px;
    display: block;
  }
  canvas.horizontal {
    width: 100%;
    height: 8px;
    min-height: 0;
  }
</style>
