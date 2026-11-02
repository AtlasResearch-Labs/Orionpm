const { spawn } = require('child_process');

// Spawns 2 long-running detached child processes to test process-tree termination
const child1 = spawn(process.execPath, ['-e', 'setInterval(()=>{}, 1000)'], { detached: true, stdio: 'ignore' });
const child2 = spawn(process.execPath, ['-e', 'setInterval(()=>{}, 1000)'], { detached: true, stdio: 'ignore' });

console.log(`[zombie_parent] PID ${process.pid} spawned child1: ${child1.pid}, child2: ${child2.pid}`);

setInterval(() => {}, 1000);

