const isEven = require("is-even");
const leftPad = require("left-pad");

const value = 42;

console.log(leftPad(String(value), 4, "0"));
console.log(`${value} is even: ${isEven(value)}`);
