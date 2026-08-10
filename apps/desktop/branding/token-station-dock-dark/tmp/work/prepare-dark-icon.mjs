import { readFileSync, writeFileSync } from "node:fs";
import { PNG } from "pngjs";

const [inputPath, outputPath] = process.argv.slice(2);
const image = PNG.sync.read(readFileSync(inputPath));

function hueAndSaturation(r, g, b) {
  const red = r / 255;
  const green = g / 255;
  const blue = b / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;
  if (delta === 0) return { hue: 0, saturation: 0 };

  let hue;
  if (max === red) hue = 60 * (((green - blue) / delta) % 6);
  else if (max === green) hue = 60 * ((blue - red) / delta + 2);
  else hue = 60 * ((red - green) / delta + 4);
  if (hue < 0) hue += 360;
  return { hue, saturation: max === 0 ? 0 : delta / max };
}

for (let offset = 0; offset < image.data.length; offset += 4) {
  const red = image.data[offset];
  const green = image.data[offset + 1];
  const blue = image.data[offset + 2];
  const alpha = image.data[offset + 3];
  if (alpha === 0) {
    image.data[offset] = 0;
    image.data[offset + 1] = 0;
    image.data[offset + 2] = 0;
    continue;
  }

  const { hue, saturation } = hueAndSaturation(red, green, blue);
  const isOrange = hue >= 5 && hue <= 48 && saturation >= 0.12 && red > green && green > blue;
  if (isOrange) continue;

  image.data[offset] = 255 - red;
  image.data[offset + 1] = 255 - green;
  image.data[offset + 2] = 255 - blue;
}

writeFileSync(outputPath, PNG.sync.write(image));
