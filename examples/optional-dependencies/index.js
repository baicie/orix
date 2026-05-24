const { familySync } = require("detect-libc");

let fseventsAvailable = false;
try {
  require.resolve("fsevents");
  fseventsAvailable = true;
} catch {
  fseventsAvailable = false;
}

console.log(`libc family: ${familySync() || "unknown"}`);
console.log(`fsevents installed: ${fseventsAvailable}`);
