use std::vec;
use std::time::Duration;
use futures::{Future, Stream, Poll, Async};
use futures_after::{After, AfterStream};
use tokio_timer::{Timer, Interval};
use web3::{self, api, Transport};
use web3::api::{Namespace, FilterStream, CreateFilter};
use web3::types::{Log, Filter, H256, Block, BlockId, BlockNumber, U256, FilterBuilder, TransactionRequest};
use web3::helpers::CallResult;
use error::{Error, ErrorKind};

pub use web3::confirm::send_transaction_with_confirmation;

pub fn logs<T: Transport>(transport: T, filter: &Filter) -> CallResult<Vec<Log>, T::Out> {
	api::Eth::new(transport).logs(filter)
}

pub fn block<T: Transport>(transport: T, id: BlockId) -> CallResult<Block<H256>, T::Out> {
	api::Eth::new(transport).block(id)
}

pub fn block_number<T: Transport>(transport: T) -> CallResult<U256, T::Out> {
	api::Eth::new(transport).block_number()
}

pub fn send_transaction<T: Transport>(transport: T, tx: TransactionRequest) -> CallResult<H256, T::Out> {
	api::Eth::new(transport).send_transaction(tx)
}

pub struct LogStreamInit {
	pub after: u64,
	pub filter: FilterBuilder,
	pub poll_interval: Duration,
	pub confirmations: usize,
}

pub struct LogStreamItem {
	pub from: u64,
	pub to: u64,
	pub logs: Vec<Log>,
}

pub enum LogStreamState<T: Transport> {
	Wait,
	FetchBlockNumber(CallResult<U256, T::Out>),
	FetchLogs {
		from: u64,
		to: u64,
		future: CallResult<Vec<Log>, T::Out>,
	},
	NextItem(Option<LogStreamItem>),
}

pub struct LogStream<T: Transport> {
	transport: T,
	interval: Interval,
	state: LogStreamState<T>,
	after: u64,
	filter: FilterBuilder,
	confirmations: usize,
}

impl<T: Transport> Stream for LogStream<T> {
	type Item = LogStreamItem;
	type Error = Error;

	fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
		loop {
			let next_state = match self.state {
				LogStreamState::Wait => {
					let _ = try_stream!(self.interval.poll());
					LogStreamState::FetchBlockNumber(block_number(&self.transport))
				},
				LogStreamState::FetchBlockNumber(ref mut future) => {
					let last_block = try_ready!(future.poll().map_err(ErrorKind::Web3)).low_u64();
					let last_confirmed_block = last_block.saturating_sub(self.confirmations as u64);
					if last_confirmed_block > self.after {
						let from = self.after + 1;
						let filter = self.filter.clone()
							.from_block(from.into())
							.to_block(last_confirmed_block.into())
							.build();
						LogStreamState::FetchLogs {
							from: from,
							to: last_confirmed_block,
							future: logs(&self.transport, &filter)
						}
					} else {
						LogStreamState::Wait
					}
				},
				LogStreamState::FetchLogs { ref mut future, from, to } => {
					let logs = try_ready!(future.poll().map_err(ErrorKind::Web3));
					let item = LogStreamItem {
						from,
						to,
						logs,
					};

					self.after = to;
					LogStreamState::NextItem(Some(item))
				},
				LogStreamState::NextItem(ref mut item) => match item.take() {
					some => return Ok(some.into()),
					None => LogStreamState::Wait,
				},
			};

			self.state = next_state;
		}
	}
}

pub fn log_stream<T: Transport>(transport: T, init: LogStreamInit) -> LogStream<T> {
	LogStream {
		transport,
		interval: Timer::default().interval(init.poll_interval),
		state: LogStreamState::Wait,
		after: init.after,
		filter: init.filter,
		confirmations: init.confirmations,
	}
}