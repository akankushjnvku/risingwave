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
import React from "react";
import { Box } from "@mui/material";
import SyntaxHighlighter from "react-syntax-highlighter";
import atomOneDarkReasonable from "react-syntax-highlighter/dist/esm/styles/hljs";

type Props = {
  nodeJson: string;
};

export default function JsonView({ nodeJson }: Props) {
  return (
    <Box width="100%" height="100%" overflow="auto">
      <SyntaxHighlighter
        language="json"
        style={atomOneDarkReasonable}
        wrapLines={true}
        showLineNumbers={false}
      >
        {nodeJson}
      </SyntaxHighlighter>
    </Box>
  );
}