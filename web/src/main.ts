import init, { BouncingBall } from "../wasm/evosim.js";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const ctx = canvas.getContext("2d");
if (!ctx) throw new Error("2d context unavailable");

function resize(): void {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.floor(window.innerWidth * dpr);
  canvas.height = Math.floor(window.innerHeight * dpr);
  canvas.style.width = `${window.innerWidth}px`;
  canvas.style.height = `${window.innerHeight}px`;
  // Reset transform when the backing store size changes.
  ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
}
window.addEventListener("resize", resize);
resize();

async function main(): Promise<void> {
  await init();
  status.textContent = "wasm loaded — milestone A demo";

  const w = window.innerWidth;
  const h = window.innerHeight;
  const ball = new BouncingBall(w, h);

  let last = performance.now();
  function frame(now: number): void {
    const dt = Math.min(64, now - last);
    last = now;
    ball.step(dt * 0.06); // ~60 px/s nominal
    ctx!.clearRect(0, 0, canvas.width, canvas.height);
    ctx!.fillStyle = "#1aa3ff";
    ctx!.beginPath();
    ctx!.arc(ball.x, ball.y, ball.radius, 0, Math.PI * 2);
    ctx!.fill();
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main().catch((err) => {
  status.textContent = `boot failed: ${err}`;
  console.error(err);
});
