import { createHash } from "node:crypto";
import {
  chmod,
  mkdir,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const repository = "takanemu/braveComicReader";
const assetName = "brave-comic-viewer-x86_64-unknown-linux-gnu";
const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const projectDirectory = dirname(scriptDirectory);
const binaryPath = join(
  projectDirectory,
  "src-tauri",
  "binaries",
  assetName,
);
const temporaryPath = `${binaryPath}.download`;

async function getLatestRelease() {
  const response = await fetch(
    `https://api.github.com/repos/${repository}/releases/latest`,
    {
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "fairy-comic-library-release-build",
      },
    },
  );
  if (!response.ok) {
    throw new Error(`GitHub release lookup failed: ${response.status}`);
  }

  const release = await response.json();
  const asset = release.assets?.find(({ name }) => name === assetName);
  if (!asset) {
    throw new Error(
      `Latest release ${release.tag_name} does not contain ${assetName}`,
    );
  }
  if (!asset.digest?.startsWith("sha256:")) {
    throw new Error(
      `Latest release asset ${assetName} does not provide a SHA-256 digest`,
    );
  }

  return { release, asset };
}

function sha256(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

async function downloadAsset(asset, expectedDigest) {
  const response = await fetch(asset.browser_download_url, {
    headers: { "User-Agent": "fairy-comic-library-release-build" },
  });
  if (!response.ok) {
    throw new Error(`Viewer download failed: ${response.status}`);
  }

  const contents = Buffer.from(await response.arrayBuffer());
  if (sha256(contents) !== expectedDigest) {
    throw new Error("Downloaded viewer SHA-256 does not match GitHub's digest");
  }
  if (
    contents.length < 20 ||
    contents.subarray(0, 4).toString("hex") !== "7f454c46" ||
    contents.readUInt16LE(18) !== 62
  ) {
    throw new Error("Downloaded viewer is not an x86_64 ELF executable");
  }

  await mkdir(dirname(binaryPath), { recursive: true });
  await writeFile(temporaryPath, contents, { mode: 0o755 });
  await chmod(temporaryPath, 0o755);
  await rename(temporaryPath, binaryPath);
}

function buildAvifenc() {
  return new Promise((resolve, reject) => {
    const child = spawn("bash", ["scripts/build-avifenc.sh"], {
      cwd: projectDirectory,
      env: { ...process.env, FORCE_BUILD_AVIFENC: "1" },
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
      } else {
        reject(
          new Error(
            `SVT-enabled avifenc build failed${signal ? ` (${signal})` : ` with exit code ${code}`}`,
          ),
        );
      }
    });
  });
}

function buildRelease() {
  return new Promise((resolve, reject) => {
    const args = ["run", "tauri", "--", "build"];
    if (process.env.TAURI_BUILD_VERBOSE) {
      args.push("--verbose");
    }
    const child = spawn("npm", args, {
      cwd: projectDirectory,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve();
      } else {
        reject(
          new Error(
            `Tauri release build failed${signal ? ` (${signal})` : ` with exit code ${code}`}`,
          ),
        );
      }
    });
  });
}

try {
  const { release, asset } = await getLatestRelease();
  const expectedDigest = asset.digest.slice("sha256:".length);
  let currentContents;
  try {
    currentContents = await readFile(binaryPath);
  } catch (error) {
    if (error.code !== "ENOENT") {
      throw error;
    }
  }

  if (currentContents && sha256(currentContents) === expectedDigest) {
    console.log(`Bundled viewer is already up to date (${release.tag_name}).`);
  } else {
    console.log(`Updating bundled viewer to ${release.tag_name}...`);
    await downloadAsset(asset, expectedDigest);
  }

  await buildAvifenc();
  await buildRelease();
} catch (error) {
  throw new Error("Release build preparation failed", { cause: error });
} finally {
  await rm(temporaryPath, { force: true });
}
