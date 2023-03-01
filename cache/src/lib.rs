//#![deny(clippy::all, clippy::pedantic, clippy::cargo)]

use dashmap::DashMap;
use atspi::{
	signify::Signified,
	accessible::{AccessibleProxy, Accessible, Role, RelationType},
	accessible_id::{HasAccessibleId, AccessibleId},
	convertable::Convertable,
	events::GenericEvent,
	text_ext::TextExt,
	InterfaceSet, StateSet,
};
use async_trait::async_trait;
use odilia_common::{errors::{AccessiblePrimitiveConversionError,OdiliaError,CacheError}, result::OdiliaResult};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, sync::Weak};
use zbus::{
	CacheProperties,
	names::OwnedUniqueName,
	zvariant::{ObjectPath, OwnedObjectPath},
	ProxyBuilder,
};

type CacheKey = AccessiblePrimitive;
type InnerCacheType = DashMap<CacheKey, CacheItem>;
type ConcurrentSafeCacheType = InnerCacheType;
type ThreadSafeCacheType = Arc<ConcurrentSafeCacheType>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
/// A struct which represents the bare minimum of an accessible for purposes of caching.
/// This makes some *possibly eronious* assumptions about what the sender is.
pub struct AccessiblePrimitive {
	/// The accessible ID in /org/a11y/atspi/accessible/XYZ; note that XYZ may be equal to any positive number, 0, "null", or "root".
	pub id: AccessibleId,
	/// Assuming that the sender is ":x.y", this stores the (x,y) portion of this sender.
	pub sender: String,
}
impl AccessiblePrimitive {
	#[allow(dead_code)]
	pub async fn into_accessible<'a>(
		self,
		conn: &zbus::Connection,
	) -> zbus::Result<AccessibleProxy<'a>> {
		let id = self.id;
		let sender = self.sender.clone();
		let path: ObjectPath<'a> = id.try_into()?;
		ProxyBuilder::new(conn).path(path)?.destination(sender)?.cache_properties(CacheProperties::No).build().await
	}
	pub fn from_event<T: GenericEvent>(event: &T) -> Result<Self, OdiliaError> {
		let sender = match event.sender() {
			Ok(Some(s)) => s,
			Ok(None) => {
				return Err(OdiliaError::PrimitiveConversionError(
					AccessiblePrimitiveConversionError::NoSender,
				))
			}
			Err(_) => {
				return Err(OdiliaError::PrimitiveConversionError(
					AccessiblePrimitiveConversionError::ErrSender,
				))
			}
		};
		let path = match event.path() {
			Some(path) => path,
			None => {
				return Err(OdiliaError::PrimitiveConversionError(
					AccessiblePrimitiveConversionError::NoPathId,
				))
			}
		};
		let id: AccessibleId = match path.try_into() {
			Ok(id) => id,
			Err(e) => return Err(OdiliaError::Zvariant(e)),
		};
		Ok(Self { id, sender: sender.to_string() })
	}
}
impl TryFrom<atspi::events::Accessible> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(
		atspi_accessible: atspi::events::Accessible,
	) -> Result<AccessiblePrimitive, Self::Error> {
		let tuple_converter = (atspi_accessible.name, atspi_accessible.path);
		tuple_converter.try_into()
	}
}
impl TryFrom<(OwnedUniqueName, OwnedObjectPath)> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(
		so: (OwnedUniqueName, OwnedObjectPath),
	) -> Result<AccessiblePrimitive, Self::Error> {
		let accessible_id: AccessibleId = so.1.try_into()?;
		Ok(AccessiblePrimitive { id: accessible_id, sender: so.0.to_string() })
	}
}
impl TryFrom<(String, OwnedObjectPath)> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(so: (String, OwnedObjectPath)) -> Result<AccessiblePrimitive, Self::Error> {
		let accessible_id: AccessibleId = so.1.try_into()?;
		Ok(AccessiblePrimitive { id: accessible_id, sender: so.0 })
	}
}
impl TryFrom<(String, AccessibleId)> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(so: (String, AccessibleId)) -> Result<AccessiblePrimitive, Self::Error> {
		Ok(AccessiblePrimitive { id: so.1, sender: so.0 })
	}
}
impl<'a> TryFrom<(String, ObjectPath<'a>)> for AccessiblePrimitive {
	type Error = OdiliaError;

	fn try_from(so: (String, ObjectPath<'a>)) -> Result<AccessiblePrimitive, Self::Error> {
		let accessible_id: AccessibleId = so.1.try_into()?;
		Ok(AccessiblePrimitive { id: accessible_id, sender: so.0 })
	}
}
impl<'a> TryFrom<&AccessibleProxy<'a>> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(accessible: &AccessibleProxy<'_>) -> Result<AccessiblePrimitive, Self::Error> {
		let sender = accessible.destination().to_string();
		let id = match accessible.id() {
			Ok(path_id) => path_id,
			Err(_) => return Err(AccessiblePrimitiveConversionError::NoPathId),
		};
		Ok(AccessiblePrimitive { id, sender })
	}
}
impl<'a> TryFrom<AccessibleProxy<'a>> for AccessiblePrimitive {
	type Error = AccessiblePrimitiveConversionError;

	fn try_from(accessible: AccessibleProxy<'_>) -> Result<AccessiblePrimitive, Self::Error> {
		let sender = accessible.destination().to_string();
		let id = match accessible.id() {
			Ok(path_id) => path_id,
			Err(_) => return Err(AccessiblePrimitiveConversionError::NoPathId),
		};
		Ok(AccessiblePrimitive { id, sender })
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
/// A struct representing an accessible. To get any information from the cache other than the stored information like role, interfaces, and states, you will need to instantiate an [`atspi::accessible::AccessibleProxy`] or other `*Proxy` type from atspi to query further info.
pub struct CacheItem {
	// The accessible object (within the application)   (so)
	pub object: AccessiblePrimitive,
	// The application (root object(?)    (so)
	pub app: AccessiblePrimitive,
	// The parent object.  (so)
	pub parent: AccessiblePrimitive,
	// The accessbile index in parent.  i
	pub index: i32,
	// Child count of the accessible  i
	pub children_num: i32,
	// The exposed interfece(s) set.  as
	pub interfaces: InterfaceSet,
	// Accessible role. u
	pub role: Role,
	// The states applicable to the accessible.  au
	pub states: StateSet,
	// The text of the accessible.
	pub text: String,
	// The children (ids) of the accessible.
	pub children: Vec<AccessiblePrimitive>,

	#[serde(skip)]
	pub cache: Weak<Cache>,
}
impl CacheItem {
	pub async fn from_atspi_event<T: Signified>(event: &T, cache: Weak<Cache>, connection: &zbus::Connection) -> OdiliaResult<Self> {
		let a11y_prim = AccessiblePrimitive::from_event(event)?;
		accessible_to_cache_item(
			&a11y_prim.into_accessible(connection).await?,
			cache
		).await
	}
	pub async fn from_atspi_cache_item(atspi_cache_item: atspi::cache::CacheItem, cache: Weak<Cache>, connection: &zbus::Connection) -> OdiliaResult<Self> {
		let children_primitives: Vec<AccessiblePrimitive> = AccessiblePrimitive::try_from(atspi_cache_item.object.clone())?
			.into_accessible(connection)
			.await?
			.get_children()
			.await?
			.into_iter()
			.map(|child_object_pair| child_object_pair.try_into())
			.collect::<Result<Vec<AccessiblePrimitive>, AccessiblePrimitiveConversionError>>()?;
		Ok(Self {
			object: atspi_cache_item.object.try_into()?,
			app: atspi_cache_item.app.try_into()?,
			parent: atspi_cache_item.parent.try_into()?,
			index: atspi_cache_item.index,
			children_num: atspi_cache_item.children,
			interfaces: atspi_cache_item.ifaces,
			role: atspi_cache_item.role,
			states: atspi_cache_item.states,
			text: atspi_cache_item.name,
			cache,
			children: children_primitives,
		})
	}
}

#[inline]
async fn as_accessible(cache_item: &CacheItem) -> OdiliaResult<AccessibleProxy<'_>> {
	let cache = strong_cache(&cache_item.cache)?;
	Ok(cache_item.object.clone().into_accessible(&cache.connection).await?)
}

#[inline]
fn strong_cache(weak_cache: &Weak<Cache>) -> OdiliaResult<Arc<Cache>> {
	Weak::upgrade(weak_cache)
		.ok_or(OdiliaError::Cache(CacheError::NotAvailable))
}

#[async_trait]
impl Accessible for CacheItem {
	type Error = OdiliaError;

	async fn get_application(&self) -> Result<Self, Self::Error> {
		let derefed_cache: Arc<Cache> = strong_cache(&self.cache)?;
		derefed_cache.get(&self.app).await.ok_or(CacheError::NoItem.into())
	}
	async fn parent(&self) -> Result<Self, Self::Error> {
		let derefed_cache: Arc<Cache> = strong_cache(&self.cache)?;
		derefed_cache.get(&self.parent).await.ok_or(CacheError::NoItem.into())
	}
	async fn get_children(&self) -> Result<Vec<Self>, Self::Error> {
		let derefed_cache: Arc<Cache> = strong_cache(&self.cache)?;
		derefed_cache.get_all(&self.children).await
			.into_iter()
			.map(|child| child.ok_or(CacheError::NoItem.into()))
			.collect()
	}
	async fn child_count(&self) -> Result<i32, Self::Error> {
		Ok(self.children_num)
	}
	async fn get_index_in_parent(&self) -> Result<i32, Self::Error> {
		Ok(self.index)
	}
	async fn get_role(&self) -> Result<Role, Self::Error> {
		Ok(self.role)
	}
	async fn get_interfaces(&self) -> Result<InterfaceSet, Self::Error> {
		Ok(self.interfaces)
	}
	async fn get_attributes(&self) -> Result<HashMap<String, String>, Self::Error> {
		Ok(as_accessible(self).await?.get_attributes().await?)
	}
	async fn name(&self) -> Result<String, Self::Error> {
		Ok(as_accessible(self).await?.name().await?)
	}
	async fn locale(&self) -> Result<String, Self::Error> {
		Ok(as_accessible(self).await?.locale().await?)
	}
	async fn description(&self) -> Result<String, Self::Error> {
		Ok(as_accessible(self).await?.description().await?)
	}
	async fn get_relation_set(&self) -> Result<Vec<(RelationType, Vec<Self>)>, Self::Error> {
		let cache = strong_cache(&self.cache)?;
		as_accessible(self).await?.get_relation_set().await?
			.into_iter()
			.map(|(relation, object_pairs)| (
				relation,
				object_pairs
					.into_iter()
					.map(|object_pair| {
							cache.get_blocking(
								&object_pair.try_into()?
							).ok_or(OdiliaError::Cache(CacheError::NoItem))
					})
					.collect::<Result<Vec<Self>, OdiliaError>>()
			))
			.map(|(relation, result_selfs)| {
				Ok((relation, result_selfs?))
			})
			.collect::<Result<Vec<(RelationType, Vec<Self>)>, OdiliaError>>()
	}
	async fn get_role_name(&self) -> Result<String, Self::Error> {
		Ok(as_accessible(self).await?.get_role_name().await?)
	}
	async fn get_state(&self) -> Result<StateSet, Self::Error> {
		Ok(self.states)
	}
	async fn get_child_at_index(&self, idx: i32) -> Result<Self, Self::Error> {
		self.get_children().await?.get(idx as usize).ok_or(CacheError::NoItem.into()).cloned()
	}
	async fn get_localized_role_name(&self) -> Result<String, Self::Error> {
		Ok(as_accessible(self).await?.get_localized_role_name().await?)
	}
	async fn accessible_id(&self) -> Result<AccessibleId, Self::Error> {
		Ok(self.object.id)
	}
}

/// The root of the accessible cache.
#[derive(Clone, Debug)]
pub struct Cache {
	pub by_id: ThreadSafeCacheType,
	pub connection: zbus::Connection,
}

/// Copy all info into a plain CacheItem struct.
/// This is very cheap, and the locking overhead will vastly outstrip making this a non-copy struct.
#[inline]
fn copy_into_cache_item(cache_item_with_handle: &CacheItem) -> CacheItem {
	CacheItem {
		object: cache_item_with_handle.object.clone(),
		parent: cache_item_with_handle.parent.clone(),
		states: cache_item_with_handle.states,
		role: cache_item_with_handle.role,
		app: cache_item_with_handle.app.clone(),
		children_num: cache_item_with_handle.children_num,
		interfaces: cache_item_with_handle.interfaces,
		index: cache_item_with_handle.index,
		text: cache_item_with_handle.text.clone(),
		children: cache_item_with_handle.children.clone(),
		cache: Weak::clone(&cache_item_with_handle.cache),
	}
}

/// An internal cache used within Odilia.
/// This contains (mostly) all accessibles in the entire accessibility tree, and they are referenced by their IDs.
/// When setting or getting information from the cache, be sure to use the most appropriate function.
/// For example, you would not want to remove individual items using the `remove()` function.
/// You should use the `remove_all()` function to acheive this, since this will only lock the cache mutex once, remove all ids, then refresh the cache.
/// If you are having issues with incorrect or invalid accessibles trying to be accessed, this is code is probably the issue.
/// This implementation is not very efficient, but it is very safe:
/// This is because before inserting, the incomming bucket is cleared (there will never be duplicate accessibles or accessibles at different states stored in the same bucket).
impl Cache {
	/// create a new, fresh cache
	pub fn new(conn: zbus::Connection) -> Self {
		Self { by_id: Arc::new(DashMap::new()), connection: conn }
	}
	/// add a single new item to the cache. Note that this will empty the bucket before inserting the `CacheItem` into the cache (this is so there is never two items with the same ID stored in the cache at the same time).
	pub async fn add(&self, cache_item: CacheItem) {
		self.by_id.insert(cache_item.object.clone(), cache_item);
	}
	/// remove a single cache item
	pub async fn remove(&self, id: &CacheKey) {
		self.by_id.remove(id);
	}
	/// get a single item from the cache (note that this copies some integers to a new struct)
	#[allow(dead_code)]
	pub async fn get(&self, id: &CacheKey) -> Option<CacheItem> {
		self.by_id.get(id).as_deref().cloned()
	}
	/// get a single item from the cache (note that this copies some integers to a new struct)
	#[allow(dead_code)]
	pub fn get_blocking(&self, id: &CacheKey) -> Option<CacheItem> {
		self.by_id.get(id).as_deref().cloned()
	}
	/// get a many items from the cache; this only creates one read handle (note that this will copy all data you would like to access)
	#[allow(dead_code)]
	pub async fn get_all(&self, ids: &[CacheKey]) -> Vec<Option<CacheItem>> {
		ids.iter()
			.map(|id| self.by_id.get(id).as_deref().cloned())
			.collect()
	}
	/// Bulk add many items to the cache; this only refreshes the cache after adding all items. Note that this will empty the bucket before inserting. Only one accessible should ever be associated with an id.
	pub async fn add_all(&self, cache_items: Vec<CacheItem>) {
		cache_items.into_iter().for_each(|cache_item| {
			self.by_id.insert(cache_item.object.clone(), cache_item);
		});
	}
	/// Bulk remove all ids in the cache; this only refreshes the cache after removing all items.
	#[allow(dead_code)]
	pub async fn remove_all(&self, ids: Vec<CacheKey>) {
		ids.iter().for_each(|id| {
			self.by_id.remove(id);
		});
	}

	/// Edit a mutable CacheItem using a function which returns the edited version.
	/// Note: an exclusive lock will be placed for the entire length of the passed function, so don't do any compute in it.
	/// Returns true if the update was successful.
	pub async fn modify_item<F>(&self, id: &CacheKey, modify: F) -> bool
	where
		F: FnOnce(&mut CacheItem),
	{
		let mut cache_item = match self.by_id.get_mut(id) {
			Some(i) => i,
			None => {
				tracing::trace!(
					"The cache does not contain the requested item: {:?}",
					id
				);
				return false;
			}
		};
		modify(&mut cache_item);
		true
	}

	/// get a single item from the cache (note that this copies some integers to a new struct).
	/// If the CacheItem is not found, create one, add it to the cache, and return it.
	pub async fn get_or_create(
		&self,
		accessible: &AccessibleProxy<'_>,
		cache: Weak<Self>,
	) -> OdiliaResult<CacheItem> {
		// if the item already exists in the cache, return it
		let primitive = accessible.try_into()?;
		if let Some(cache_item) = self
			.get(&primitive)
			.await
		{
			return Ok(cache_item);
		}
		// otherwise, build a cache item
		let start = std::time::Instant::now();
		let cache_item = accessible_to_cache_item(accessible, cache).await?;
		let end = std::time::Instant::now();
		let diff = end - start;
		tracing::debug!("Time to create cache item: {:?}", diff);
		// add a clone of it to the cache
		self.add(copy_into_cache_item(&cache_item)).await;
		// return that same cache item
		Ok(cache_item)
	}
}

pub async fn accessible_to_cache_item(accessible: &AccessibleProxy<'_>, cache: Weak<Cache>) -> OdiliaResult<CacheItem> {
	let (app, parent, index, children_num, interfaces, role, states, children) = tokio::try_join!(
		accessible.get_application(),
		accessible.parent(),
		accessible.get_index_in_parent(),
		accessible.child_count(),
		accessible.get_interfaces(),
		accessible.get_role(),
		accessible.get_state(),
		accessible.get_children(),
	)?;
	// if it implements the Text interface
	let text = match accessible.to_text().await {
		// get *all* the text
		Ok(text_iface) => text_iface.get_all_text().await,
		// otherwise, use the name instaed
		Err(_) => Ok(accessible.name().await?),
	}?;
	Ok(CacheItem {
		object: accessible.try_into()?,
		app: app.try_into()?,
		parent: parent.try_into()?,
		index,
		children_num,
		interfaces,
		role,
		states,
		text,
		children: children.into_iter()
			.map(AccessiblePrimitive::try_from)
			.collect::<Result<Vec<AccessiblePrimitive>, _>>()?,
		cache,
	})
}
