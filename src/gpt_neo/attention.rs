// Copyright 2021 The Eleuther AI and HuggingFace Inc. team. All rights reserved.
// Copyright 2021 Guillaume Becquin
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::common::dropout::Dropout;
use crate::RustBertError;
use tch::{Kind, Tensor};

trait GptNeoAttention {
    fn get_block_length_and_num_blocks(sequence_length: i64, window_size: i64) -> (i64, i64) {
        let mut block_length = window_size;
        while sequence_length % block_length != 0 {
            block_length -= 1;
        }
        let num_blocks = sequence_length / block_length;
        (block_length, num_blocks)
    }

    fn look_back(
        input_tensor: &Tensor,
        block_length: i64,
        window_size: i64,
        pad_value: Option<i64>,
        is_key_value: bool,
    ) -> Result<Tensor, RustBertError> {
        let padding_size = match input_tensor.size().len() {
            3 => Vec::from([0, 0, window_size, 0]),
            2 => Vec::from([window_size, 0]),
            _ => {
                return Err(RustBertError::ValueError(format!(
                    "Invalid tensor rank, expected 2 or 3, got {}",
                    input_tensor.size().len()
                )));
            }
        };

        let mut padded_tensor = match pad_value {
            None => input_tensor.constant_pad_nd(padding_size.as_slice()),
            Some(value) => {
                if value == 0 {
                    input_tensor.constant_pad_nd(padding_size.as_slice())
                } else {
                    (input_tensor - value).constant_pad_nd(padding_size.as_slice()) + value
                }
            }
        };

        padded_tensor = padded_tensor.unfold(1, window_size + block_length, block_length);
        if is_key_value {
            padded_tensor = padded_tensor.transpose(-2, -1);
        }

        Ok(padded_tensor)
    }

    fn split_heads(
        input_tensor: &Tensor,
        num_heads: i64,
        attention_head_size: i64,
    ) -> Result<Tensor, RustBertError> {
        let mut new_shape = input_tensor.size();
        let _ = new_shape.pop();
        new_shape.extend_from_slice(&[num_heads, attention_head_size]);

        let reshaped_tensor = input_tensor.view(new_shape.as_slice());

        Ok(match reshaped_tensor.size().len() {
            5 => reshaped_tensor.permute(&[0, 1, 3, 2, 4]),
            4 => reshaped_tensor.permute(&[0, 2, 1, 3]),
            _ => {
                return Err(RustBertError::ValueError(format!(
                    "Invalid tensor rank, expected 4 or 5, got {}",
                    input_tensor.size().len()
                )));
            }
        })
    }

    fn merge_heads(
        input_tensor: &Tensor,
        num_heads: i64,
        attention_head_size: i64,
    ) -> Result<Tensor, RustBertError> {
        let output_tensor = match input_tensor.size().len() {
            5 => input_tensor.permute(&[0, 1, 3, 2, 4]).contiguous(),
            4 => input_tensor.permute(&[0, 2, 1, 3]).contiguous(),
            _ => {
                return Err(RustBertError::ValueError(format!(
                    "Invalid tensor rank, expected 4 or 5, got {}",
                    input_tensor.size().len()
                )));
            }
        };
        let mut new_shape = input_tensor.size();
        new_shape.truncate(new_shape.len() - 2);
        new_shape.push(num_heads * attention_head_size);
        Ok(output_tensor.view(new_shape.as_slice()))
    }

    fn split_sequence_length_dim_to(
        input_tensor: &Tensor,
        dim_factor_1: i64,
        dim_factor_2: i64,
        hidden_size: i64,
    ) -> Result<Tensor, RustBertError> {
        let batch_size = input_tensor.size()[0];
        let mut split_dim_shape = Vec::from([batch_size, dim_factor_1, dim_factor_2]);

        Ok(match input_tensor.size().len() {
            3 => {
                split_dim_shape.push(hidden_size);
                input_tensor.reshape(split_dim_shape.as_slice())
            }
            2 => input_tensor.reshape(split_dim_shape.as_slice()),
            _ => {
                return Err(RustBertError::ValueError(format!(
                    "Invalid tensor rank, expected 2 or 3, got {}",
                    input_tensor.size().len()
                )));
            }
        })
    }

    fn attend(
        query: &Tensor,
        key: &Tensor,
        value: &Tensor,
        causal_mask: &Tensor,
        masked_bias: &Tensor,
        attention_dropout: &Dropout,
        attention_mask: Option<&Tensor>,
        train: bool,
    ) -> (Tensor, Tensor) {
        let mut attention_weights = query
            .matmul(&key.transpose(-1, -2))
            .where1(causal_mask, masked_bias);

        if let Some(attention_mask_value) = attention_mask {
            attention_weights = attention_weights + attention_mask_value;
        };

        attention_weights = attention_weights
            .softmax(-1, Kind::Float)
            .apply_t(attention_dropout, train);

        let attention_output = attention_weights.matmul(value);
        (attention_output, attention_weights)
    }
}
