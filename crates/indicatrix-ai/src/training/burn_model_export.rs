//! One-transfer export from the trainable Burn model to the dependency-light pure model.

use burn::{
    nn::{LayerNorm, Linear},
    prelude::*,
};

use super::{
    TrainError,
    burn_model::{BurnSetAttention, host_vec},
};
use crate::model::{GemstoneSetAttentionNetwork, LayerNorm as PureLayerNorm, Linear as PureLinear};

/// Append every trainable model parameter in a stable order, then read them all back together.
fn append_linear(layer: &Linear, tensors: &mut Vec<Tensor<1>>) {
    let [input, output] = layer.weight.val().dims();
    tensors.push(layer.weight.val().reshape([input * output]));
    if let Some(bias) = &layer.bias {
        tensors.push(bias.val());
    }
}

fn append_norm(layer: &LayerNorm, tensors: &mut Vec<Tensor<1>>) {
    tensors.push(layer.gamma.val());
    if let Some(beta) = &layer.beta {
        tensors.push(beta.val());
    }
}

struct WeightReader {
    flat: Vec<f32>,
    cursor: usize,
}

impl WeightReader {
    fn take(&mut self, count: usize, what: &str) -> Result<Vec<f32>, TrainError> {
        let end = self.cursor.checked_add(count).ok_or_else(|| {
            TrainError::ShapeMismatch(format!("{what}: export parameter count overflow"))
        })?;
        let values = self.flat.get(self.cursor..end).ok_or_else(|| {
            TrainError::ShapeMismatch(format!(
                "{what}: packed export ended at {} while reading {count} values",
                self.cursor
            ))
        })?;
        self.cursor = end;
        Ok(values.to_vec())
    }

    fn linear(&mut self, layer: &Linear, what: &str) -> Result<PureLinear, TrainError> {
        let [input, output] = layer.weight.val().dims();
        let read_weights = self.take(input * output, what)?;
        let mut weights = vec![0.0; read_weights.len()];
        for inp in 0..input {
            for out in 0..output {
                weights[out * input + inp] = read_weights[inp * output + out];
            }
        }
        let biases = match &layer.bias {
            Some(_) => self.take(output, what)?,
            None => vec![0.0; output],
        };
        if biases.len() != output {
            return Err(TrainError::ShapeMismatch(format!(
                "{what}: read {} biases for {output} outputs",
                biases.len()
            )));
        }
        Ok(PureLinear {
            weights,
            biases,
            in_features: input,
            out_features: output,
        })
    }

    fn norm(
        &mut self,
        layer: &LayerNorm,
        eps: f32,
        what: &str,
        width: usize,
    ) -> Result<PureLayerNorm, TrainError> {
        let gamma = self.take(width, what)?;
        let beta = if layer.beta.is_some() {
            self.take(width, what)?
        } else {
            vec![0.0; width]
        };
        Ok(PureLayerNorm { gamma, beta, eps })
    }

    fn finish(self) -> Result<(), TrainError> {
        if self.cursor == self.flat.len() {
            Ok(())
        } else {
            Err(TrainError::ShapeMismatch(format!(
                "packed export has {} unread parameter values",
                self.flat.len() - self.cursor
            )))
        }
    }
}

impl BurnSetAttention {
    /// Exports trained Burn weights to the pure model with one packed host readback.
    ///
    /// The caller keeps its previous network if this conversion fails.
    pub fn export_to_set_attention(&self) -> Result<GemstoneSetAttentionNetwork, TrainError> {
        let mut tensors = Vec::new();
        append_linear(&self.tier_proj, &mut tensors);
        append_linear(&self.global_proj, &mut tensors);
        for layer in 0..self.num_layers {
            for linear in [
                &self.attn_q[layer],
                &self.attn_k[layer],
                &self.attn_v[layer],
                &self.attn_out[layer],
            ] {
                append_linear(linear, &mut tensors);
            }
            append_norm(&self.pre_attn_norms[layer], &mut tensors);
            append_norm(&self.post_attn_norms[layer], &mut tensors);
            append_linear(&self.mlp_fc1[layer], &mut tensors);
            append_linear(&self.mlp_fc2[layer], &mut tensors);
        }
        append_norm(&self.final_norm, &mut tensors);
        append_linear(&self.critic_head, &mut tensors);
        append_linear(&self.sens_head, &mut tensors);
        append_linear(&self.gain_head, &mut tensors);
        tensors.push(self.log_std_raw.val());

        let flat = host_vec(Tensor::cat(tensors, 0))?;
        let mut reader = WeightReader { flat, cursor: 0 };
        let tier_proj = reader.linear(&self.tier_proj, "tier_proj")?;
        let global_proj = reader.linear(&self.global_proj, "global_proj")?;
        let mut attn_layers = Vec::with_capacity(self.num_layers);
        let mut pre_attn_norms = Vec::with_capacity(self.num_layers);
        let mut post_attn_norms = Vec::with_capacity(self.num_layers);
        let mut mlp_blocks = Vec::with_capacity(self.num_layers);
        for layer in 0..self.num_layers {
            attn_layers.push(crate::model::MultiHeadAttention {
                q_proj: reader.linear(&self.attn_q[layer], "attention query")?,
                k_proj: reader.linear(&self.attn_k[layer], "attention key")?,
                v_proj: reader.linear(&self.attn_v[layer], "attention value")?,
                out_proj: reader.linear(&self.attn_out[layer], "attention output")?,
                dim: self.d_model,
                num_heads: self.num_heads,
            });
            pre_attn_norms.push(reader.norm(
                &self.pre_attn_norms[layer],
                self.pre_attn_eps[layer],
                "pre-attention norm",
                self.d_model,
            )?);
            post_attn_norms.push(reader.norm(
                &self.post_attn_norms[layer],
                self.post_attn_eps[layer],
                "post-attention norm",
                self.d_model,
            )?);
            mlp_blocks.push(crate::model::MlpBlock {
                fc1: reader.linear(&self.mlp_fc1[layer], "MLP input")?,
                fc2: reader.linear(&self.mlp_fc2[layer], "MLP output")?,
            });
        }
        let final_norm =
            reader.norm(&self.final_norm, self.final_eps, "final norm", self.d_model)?;
        let critic_head = reader.linear(&self.critic_head, "critic head")?;
        let sens_head = reader.linear(&self.sens_head, "sensitivity head")?;
        let gain_head = reader.linear(&self.gain_head, "gain head")?;
        let log_std_raw = <[f32; crate::model::LOG_STD_SLOTS]>::try_from(
            reader.take(crate::model::LOG_STD_SLOTS, "log standard deviation")?,
        )
        .map_err(|values| {
            TrainError::ShapeMismatch(format!(
                "log_std_raw: read back {} values, expected {}",
                values.len(),
                crate::model::LOG_STD_SLOTS
            ))
        })?;
        reader.finish()?;
        let network = GemstoneSetAttentionNetwork {
            tier_proj,
            global_proj,
            attn_layers,
            pre_attn_norms,
            post_attn_norms,
            mlp_blocks,
            final_norm,
            critic_head,
            sens_head,
            gain_head,
            log_std_raw,
            d_model: self.d_model,
            num_heads: self.num_heads,
            d_tier: self.d_tier,
            d_global: self.d_global,
        };
        network.validate().map_err(|error| {
            TrainError::ShapeMismatch(format!("exported pure network failed validation: {error}"))
        })?;
        Ok(network)
    }
}
