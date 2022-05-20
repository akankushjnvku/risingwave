/*
 * Copyright 2022 Singularity Data
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 */
import { newMatrix } from "./util";

/**
 * Traverse a tree from its root node, and do operation
 * by calling the step function.
 * Every node will be visted only once.
 * @param {{nextNodes: []}} root The root node of the tree
 * @param {(node: any) => boolean} step callback when a node is visited.
 * return true if you want to stop to traverse its next nodes.
 */
export function treeBfs(root: any, step: Function) {
  const queue = [root];
  while (queue.length) {
    const c = queue.shift();

    if (!step(c)) {
      for (const nextNode of c.nextNodes) {
        queue.push(nextNode);
      }
    }
  }
}

/**
 * Traverse a graph from a random node, and do
 * operation by calling the step function.
 * Every node will be visted only once.
 * @param {{nextNodes: []}} root A random node in the graph
 * @param {(node: any) => boolean} step callback when a node is visited.
 * @param {string} [neighborListKey="nextNodes"]
 * return true if you want to stop traverse its next nodes
 */
export function graphBfs(root: any, step: Function, neighborListKey?: any) {
  const key = neighborListKey || "nextNodes";
  const visitedNodes = new Set();
  const queue = [root];
  while (queue.length) {
    const c = queue.shift();

    visitedNodes.add(c);
    if (!step(c)) {
      for (const nextNode of c[key]) {
        if (!visitedNodes.has(nextNode)) {
          queue.push(nextNode);
        }
      }
    }
  }
}

/**
 * Group nodes in the same connected component. The method will not
 * change the input. The output contains the original references.
 * @param {Array<{nextNodes: []}>} nodes
 * @returns {Array<Array<any>>} A list of groups containing
 * nodes in the same connected component
 */
export function getConnectedComponent(nodes: any) {
  const node2shellNodes = new Map();

  for (const node of nodes) {
    const shellNode = {
      val: node,
      nextNodes: [],
      g: -1,
    };
    node2shellNodes.set(node, shellNode);
  }

  // make a shell non-directed graph from the original DAG.
  for (const node of nodes) {
    const shellNode = node2shellNodes.get(node);
    for (const nextNode of node.nextNodes) {
      const nextShellNode = node2shellNodes.get(nextNode);
      shellNode.nextNodes.push(nextShellNode);
      nextShellNode.nextNodes.push(shellNode);
    }
  }

  // bfs assign group number
  let cnt = 0;
  for (let node of node2shellNodes.keys()) {
    let shellNode = node2shellNodes.get(node);
    if (shellNode.g === -1) {
      shellNode.g = cnt++;
      graphBfs(shellNode, (c: any) => {
        c.g = shellNode.g;
        return false;
      });
    }
  }

  const group = newMatrix(cnt);
  for (const node of node2shellNodes.keys()) {
    const shellNode = node2shellNodes.get(node);
    group[shellNode.g].push(node);
  }

  return group;
}